/**
 * Worf 0.0.1
 * Copyright (c) 2025 Ari Rios <me@aririos.com>
 * License-SPDX: GPL-3.0-only
 * Based on Polochon-street/blissify-rs
 * 
 * Worf is a daemon that automatically queues songs in MPD based off a "pinned song", which is just whatever song was playing when the daemon was started.
 * It uses bliss-audio to queue songs that are most similar to the pinned song. The pinned song is changed simply by playing a new song.
 * 
 * TODO:
 * - Move beyond just using bliss and integrate last.fm similar artists and/or genre tags. 
 * - In a separate thread, keep the bliss database updated as new songs are added to MPD.
 * - Ultimately, the idea of keeping state about the "pinned song" requires a whole new MPD client -- none of the current ones have an idea of "song radio", "artist radio", etc.
 */

pub mod ffmpeg_decoder;
use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::AnalysisOptions;
use bliss_audio::library::{AppConfigTrait, BaseConfig, Library, LibrarySong as BlissSong};
use bliss_audio::playlist::{closest_to_songs, euclidean_distance, DistanceMetricBuilder};
use clap::{Parser, Subcommand};
// use extended_isolation_forest::ForestOptions;
use fallible_streaming_iterator::FallibleStreamingIterator;
use ffmpeg_decoder::FFmpegDecoder as Decoder;
use mpd::song::QueuePlace;
use mpd::{Client, Idle, Query, Song as MPDSong, Term, search::Window};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{env, io};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 10)]
    queue_length: u8,
    #[arg(long, default_value_t = 100)]
    history_length: u8,
    #[arg(short, long)]
    base_path: Option<PathBuf>,
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    #[arg(short, long)]
    database_path: Option<PathBuf>,
    #[arg(short, long)]
    password: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Daemon,
    Update,
    Init,
}

/// The main struct which holds the [bliss_audio::library::Library] and [mpd::Client].
struct MPDLibrary {
    bliss: Library<Config, Decoder>,
    mpd_conn: Arc<Mutex<Client<MPDStream>>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Config {
    #[serde(flatten)]
    base_config: BaseConfig,
    mpd_base_path: PathBuf,
}

impl AppConfigTrait for Config {
    fn base_config(&self) -> &BaseConfig {
        &self.base_config
    }
    fn base_config_mut(&mut self) -> &mut BaseConfig {
        &mut self.base_config
    }
}

impl Config {
    fn build(
        mpd_base_path: PathBuf,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
        analysis_options: Option<AnalysisOptions>,
    ) -> Result<Self> {
        let base_config = BaseConfig::new(config_path, database_path, analysis_options)?;
        Ok(Self {
            base_config,
            mpd_base_path,
        })
    }
}

// If this were just `trait Duplex: Read + Write {}` and
// `struct MPDStream(Box<dyn Duplex>)`, we would have to box up every
// backing stream type and implement `From` so that the `?`
// operator understands how to turn each backing type
// into the struct type. `enum_dispatch` can solve this but don't
// want to pull in a whole new crate for this one enum
enum MPDStream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Read for MPDStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            MPDStream::Tcp(s) => s.read(buf),
            MPDStream::Unix(s) => s.read(buf),
        }
    }
}

impl Write for MPDStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            MPDStream::Tcp(s) => s.write(buf),
            MPDStream::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            MPDStream::Tcp(s) => s.flush(),
            MPDStream::Unix(s) => s.flush(),
        }
    }
}

impl MPDLibrary {
    fn connect_to_mpd() -> Result<Client<MPDStream>> {
        let (password, mpd_host) = match env::var("MPD_HOST") {
            Ok(h) => match h.split_once('@') {
                // Only host
                None => (None, h),
                // Unix socket
                Some(("", _)) => (None, h),
                // Password + host
                Some((password, host)) => (Some(password.to_owned()), host.to_owned()),
            },
            Err(_) => {
                eprintln!("MPD_HOST not set in the environment, defaulting to 127.0.0.1");
                (None, String::from("127.0.0.1"))
            }
        };

        let mpd_port = match env::var("MPD_PORT") {
            Ok(p) => p
                .parse::<u16>()
                .with_context(|| "while trying to coerce MPD_PORT to int")?,
            Err(_) => {
                // Would prefer to defer to MPD for the default port but the mpd crate doesn't have a method for doing that
                eprintln!("MPD_PORT not set in the environment, using default of 6600");
                6600
            }
        };

        let mut client: Client<MPDStream> = {
            if mpd_host.starts_with('/') || mpd_host.starts_with('~') {
                Client::new(MPDStream::Unix(UnixStream::connect(mpd_host)?))?
            } else if mpd_host.starts_with('@') {
                let addr = SocketAddr::from_abstract_name(mpd_host.split_once('@').unwrap().1)?;
                Client::new(MPDStream::Unix(UnixStream::connect_addr(&addr)?))?
            } else {
                Client::new(MPDStream::Tcp(TcpStream::connect(format!(
                    "{}:{}",
                    mpd_host, mpd_port
                ))?))?
            }
        };
        if let Some(pass) = password {
            client.login(&pass)?;
        }
        Ok(client)
    }

    fn build(
        mpd_base_path: PathBuf,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
    ) -> Result<Self> {
        let config = Config::build(mpd_base_path, config_path, database_path, None)?;
        Ok(Self {
            bliss: Library::new(config)?,
            mpd_conn: Arc::new(Mutex::new(Self::connect_to_mpd()?)),
        })
    }

    fn retrieve(config_path: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            bliss: Library::from_config_path(config_path)?,
            mpd_conn: Arc::new(Mutex::new(Self::connect_to_mpd()?)),
        })
    }

    fn get_all_mpd_songs(&self) -> Result<Vec<String>> {
        let mut songs = vec![];
        let mut query = Query::new();
        let query = query.and(Term::File, "");
        let (mut index, chunk_size) = (0, 10000);
        loop {
            let search = self
                .mpd_conn
                .lock()
                .unwrap()
                .search(query, Window::from((index, index + chunk_size)))?;
            if search.is_empty() {
                break;
            }
            songs.extend(search.into_iter().map(|s| s.file.to_owned()).map(|s| {
                String::from(
                    Path::new(&self.bliss.config.mpd_base_path)
                        .join(Path::new(&s))
                        .to_str()
                        .unwrap(), // won't panic since both the MPD base path and song file path are already valid Unicode
                )
            }));
            index += chunk_size;
            songs.sort();
            songs.dedup();
        }
        Ok(songs)
    }

    fn populate(&mut self) -> Result<()> {
        let sqlite_conn = self.bliss.sqlite_conn.lock().unwrap();
        let mut songs_query = sqlite_conn.prepare("select * from song")?;
        let songs_result = songs_query.query([])?;
        let songs_total = songs_result.count()?;
        if songs_total > 0 {
            loop {
                println!("Database contains data ({songs_total} songs). Continue anyway? (Y/N)");
                let mut answer = String::new();
                io::stdin()
                    .read_line(&mut answer)
                    .expect("Failed to read answer");
                let answer: char = match answer.trim().to_uppercase().parse::<char>() {
                    Ok(ch) => ch,
                    Err(_) => continue,
                };
                match answer {
                    'Y' => {
                        println!("Continuing...");
                        break;
                    }
                    'N' => {
                        println!("Aborting!");
                        bail!("User aborted.");
                    }
                    _ => println!("Try again."),
                }
            }
        }
        drop(songs_query);
        drop(sqlite_conn);
        self.bliss.analyze_paths(self.get_all_mpd_songs()?, true)
    }

    fn bliss_song_to_mpd(&self, song: &BlissSong<()>) -> Result<MPDSong> {
        let path = song.bliss_song.path.to_owned();
        let path = path.strip_prefix(&*self.bliss.config.mpd_base_path.to_string_lossy())?;
        Ok(MPDSong {
            file: path.to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    fn queue_from_song<'a, F, I>(
        &self,
        song: &MPDSong,
        num_songs: usize,
        distance: &'a dyn DistanceMetricBuilder,
        sort_by: F,
        dedup: bool,
        keep_queue: bool,
    ) -> Result<MPDSong>
    where
        F: Fn(&[BlissSong<()>], &[BlissSong<()>], &'a dyn DistanceMetricBuilder) -> I,
        I: Iterator<Item = BlissSong<()>> + 'a,
    {
        let mut mpd_conn = self.mpd_conn.lock().unwrap();
        let path = self.bliss.config.mpd_base_path.join(&song.file);
        let mut playlist = self
            .bliss
            .playlist_from_custom(&[&path.to_string_lossy().clone()], distance, sort_by, dedup)?
            .skip(1);

        let mut last_status = mpd_conn.status()?;
        let current_pos = song.place.unwrap().pos;
        if !keep_queue {
            mpd_conn.delete(0..current_pos)?;
            if mpd_conn.queue()?.len() > 1 {
                mpd_conn.delete(1..)?;
            }
        }

        for _ in 0..num_songs {
            let mpd_song = self.bliss_song_to_mpd(playlist.next().as_ref().unwrap())?;
            mpd_conn.push(mpd_song)?;
        }

        for bliss_song in playlist {
            let next_event = mpd_conn.wait(&[mpd::Subsystem::Queue])?;

            if next_event[0] == mpd::Subsystem::Queue {
                let new_status = mpd_conn.status()?;
                let last_song = last_status.song.unwrap_or(QueuePlace {pos: u32::MAX, ..Default::default()});
                let new_song = new_status.song.unwrap_or(QueuePlace { pos: u32::MAX, ..Default::default()});
                // FIXME: this error is probably caused by a race condition around adding the next song to the queue
                if new_song.pos == u32::MAX || last_song.pos == u32::MAX { 
                    println!("Failed to get next song in MPD queue, continuing with same pinned song...");
                    continue;
                }
                // TODO: change to start queueing when reaching the end
                if new_song.pos == last_song.pos + 1 {
                    let mpd_song = self.bliss_song_to_mpd(&bliss_song)?;
                    mpd_conn.push(mpd_song)?;
                    last_status = new_status;
                } else if new_song.id == last_song.id {
                    last_status = new_status;
                    continue;
                } else if new_status.state == mpd::status::State::Play {
                    return Ok(mpd_conn.currentsong()?.unwrap());
                }
            }
        }

        Ok(song.clone())
    }
}

struct PinnedSong(MPDSong);

fn main() -> Result<()> {
    let args = Args::parse();

    match &args.command {
        Some(Commands::Daemon) => {
            println!("Queueing {} songs and daemonizing...", args.queue_length);
            let config_path = args.config_path;
            let mpd_library = match MPDLibrary::retrieve(config_path.clone()) {
                Ok(library) => library,
                Err(e) => match e.downcast::<std::io::Error>() {
                    Ok(inner) => match inner.kind() {
                        std::io::ErrorKind::NotFound => {
                            let database_path: Option<PathBuf> = None;
                            MPDLibrary::build(args.base_path.ok_or(anyhow!("--base-path must be used if running `daemon` without an initialized library"))?, config_path, database_path)?
                        }
                        kind => bail!("Could not retrieve or create MPDLibrary, io error: {kind}"),
                    },
                    Err(inner_err) => bail!(
                        "Could not retrieve or create MPDLibrary (failed to downcast MPDLibrary::retrieve error: {inner_err})"
                    ),
                },
            };

            let Ok(Some(current_song)) = mpd_library.mpd_conn.lock().unwrap().currentsong() else {
                bail!("Start playing a song and try again.");
            };

            let mut pinned_song = PinnedSong(current_song);

            println!("Queueing from pinned song: {}", pinned_song.0.title.as_ref().unwrap());

            let sort = |x: &[BlissSong<()>],
                        y: &[BlissSong<()>],
                        z|
             -> Box<dyn Iterator<Item = BlissSong<()>>> {
                Box::new(closest_to_songs(x, y, z))
            };

            // let forest_distance: &dyn DistanceMetricBuilder = &ForestOptions {
            //     n_trees: 1000,
            //     sample_size: 200,
            //     max_tree_depth: None,
            //     extension_level: 10,
            // };

            loop {
                match mpd_library.queue_from_song(
                    &pinned_song.0,
                    10,
                    &euclidean_distance,
                    sort,
                    true,
                    false,
                ) {
                    Ok(next_pinned) => {
                        if next_pinned == pinned_song.0 {
                            println!("You made it to the end of your music library!");
                            return Ok(());
                        }
                        pinned_song = PinnedSong(next_pinned);
                        println!("Restarting with new pinned song: {}", pinned_song.0.title.as_ref().unwrap());
                        continue;
                    }
                    Err(e) => bail!(e),
                }
            }
        }
        Some(Commands::Init) => {
            println!("Initializing music library and analyzing...");
            let config_path = args.config_path;
            let database_path = args.database_path;
            let Some(mpd_base_path) = args.base_path else {
                bail!("--base-path must be used if running `init`");
            };
            let mut mpd_library = MPDLibrary::build(mpd_base_path, config_path, database_path)?;
            mpd_library.populate()
        }
        Some(Commands::Update) => {
            println!("Updating music library analysis...");
            let config_path = args.config_path;
            let mut mpd_library = MPDLibrary::retrieve(config_path)?;
            mpd_library
                .bliss
                .update_library(mpd_library.get_all_mpd_songs()?, true, true)
        }
        None => {
            panic!("No command provided!");
        }
    }
}
