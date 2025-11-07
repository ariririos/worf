mod ffmpeg_decoder;

use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::{
    AnalysisOptions, Song as BareBlissSong,
    library::{AppConfigTrait, BaseConfig, Library, LibrarySong as BlissSong},
    playlist::DistanceMetricBuilder,
};
use fallible_streaming_iterator::FallibleStreamingIterator;
use ffmpeg_decoder::FFmpegDecoder as Decoder;
use mpd::{Client, Idle, Query, Song as MPDSong, Term, search::Window};
use ndarray::{Array1, arr1};
use noisy_float::prelude::n32;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// If this were just `trait Duplex: Read + Write {}` and
// `struct MPDStream(Box<dyn Duplex>)`, I would have to box up every
// backing stream type and implement `From` so that the `?`
// operator understands how to turn each backing type
// into the struct type. `enum_dispatch` can solve this but don't
// want to pull in a whole new crate for this one enum
pub enum MPDStream {
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
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
        let base_config = BaseConfig::new(config_path, database_path, analysis_options)
            .context("while creating bliss BaseConfig")?;
        Ok(Self {
            base_config,
            mpd_base_path,
        })
    }
}

fn average(v: &[f32]) -> f32 {
    if !v.is_empty() {
        v.iter().sum::<f32>() / v.len() as f32
    } else {
        0.0
    }
}

fn vec_avg<const N: usize>(v: Vec<Vec<f32>>) -> Vec<f32> {
    (0..N)
        .map(|i| average(&v.iter().map(|arr| arr[i]).collect::<Vec<_>>()))
        .collect()
}

pub fn collapse_genres(genre_weights: &GenreWeights, genres: String) -> Vec<f32> {
    if genres.is_empty() {
        vec![0.0; 5]
    } else {
        vec_avg::<5>(
            genres
                .split(",")
                .filter_map(|genre| genre_weights.get(genre))
                .map(|weights| weights.to_owned())
                .collect::<Vec<_>>(),
        )
    }
}

/// A modified version of the bliss default playlist creator, sorting by genre weights.
pub fn closest_to_genre_songs<'a, T: AsRef<BareBlissSong> + Clone + 'a>(
    initial_songs: &[T],
    candidate_songs: &[T],
    metric_builder: &'a dyn DistanceMetricBuilder,
    track_weights: &GenreWeights,
) -> impl Iterator<Item = T> + 'a {
    let initial_songs: Vec<Array1<f32>> = initial_songs
        .iter()
        .filter_map(|c| {
            Some(arr1(
                track_weights.get(&*c.as_ref().path.to_string_lossy())?,
            ))
        })
        .collect::<Vec<_>>();
    let metric = metric_builder.build(&initial_songs);
    let mut candidate_songs = candidate_songs.to_vec();
    candidate_songs.sort_by_cached_key(|song| {
        n32(metric.distance(&arr1(
            track_weights
                .get(&*song.as_ref().path.to_string_lossy())
                .unwrap_or(&vec![0.0; 5]),
        )))
    });
    candidate_songs.into_iter()
}

/// Gets a reference to the underlying array.
///
/// If `N` is not exactly equal to the length of `v`, then this function returns `None`.
/// Copied from rust/library/core/src/slice/mod.rs
#[inline]
#[must_use]
pub const fn slice_as_array<const N: usize, T>(v: &[T]) -> Option<&[T; N]> {
    if v.len() == N {
        let ptr = v.as_ptr().cast();

        // SAFETY: The underlying array of a slice can be reinterpreted as an actual array `[T; N]` if `N` is not greater than the slice's length.
        let me = unsafe { &*ptr };
        Some(me)
    } else {
        None
    }
}

pub type GenreWeights = HashMap<String, Vec<f32>>;
type TrackWeights = HashMap<String, Vec<f32>>;
/// The main struct which holds the [bliss_audio::library::Library] and [mpd::Client]. Also holds the genre weightings ([GenreWeights]) if present.
pub struct MPDLibrary {
    pub bliss: Library<Config, Decoder>,
    pub mpd_conn: Arc<Mutex<Client<MPDStream>>>,
    pub genre_weights: Option<GenreWeights>,
}

/// MPDLibrary holds the connection to MPD, methods to analyze songs with bliss, and the main `queue_from_song` method
/// that does the queueing of similar songs.
impl MPDLibrary {
    /// connect_to_mpd doesn't need to be called directly, building or retrieving an existing MPDLibrary
    /// will do it for you.
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
                .context("while trying to coerce MPD_PORT to int")?,
            Err(_) => {
                // Would prefer to defer to MPD for the default port but the mpd crate doesn't have a method for doing that
                eprintln!("MPD_PORT not set in the environment, using default of 6600");
                6600
            }
        };

        let mut client: Client<MPDStream> = {
            if mpd_host.starts_with('/') || mpd_host.starts_with('~') {
                Client::new(MPDStream::Unix(
                    UnixStream::connect(mpd_host).context("while connecting to Unix stream")?,
                ))
                .context("while connecting to MPD")?
            } else if mpd_host.starts_with('@') {
                let addr = SocketAddr::from_abstract_name(
                    mpd_host
                        .split_once('@')
                        .ok_or(anyhow!("No socket name provided"))?
                        .1,
                )?;
                Client::new(MPDStream::Unix(
                    UnixStream::connect_addr(&addr).context("while connecting to Unix stream")?,
                ))
                .context("while connecting to MPD")?
            } else {
                Client::new(MPDStream::Tcp(
                    TcpStream::connect(format!("{}:{}", mpd_host, mpd_port))
                        .context("while connecting to TCP stream")?,
                ))
                .context("while connecting to MPD")?
            }
        };
        if let Some(pass) = password {
            client.login(&pass).context("while logging in to MPD")?;
        }
        Ok(client)
    }

    /// Build a new MPDLibrary. May fail if paths provided don't exist or if an error occurs connecting to MPD.
    /// If no paths are provided for `config_path` or `database_path`, bliss will default to locations in
    /// $XDG_CONFIG_HOME.
    pub fn build(
        mpd_base_path: PathBuf,
        config_path: Option<PathBuf>,
        database_path: Option<PathBuf>,
    ) -> Result<Self> {
        let config = Config::build(mpd_base_path, config_path, database_path, None).context("while building bliss Config")?;
        Ok(Self {
            bliss: Library::new(config).context("while building bliss library")?,
            mpd_conn: Arc::new(Mutex::new(Self::connect_to_mpd().context("while connecting to MPD")?)),
            genre_weights: None,
        })
    }

    /// Retrieve an existing MPDLibrary from disk. May fail if path provided doesn't exist or if an error occurs
    /// connecting to MPD. If no path is provided, bliss will look up a configuration in $XDG_CONFIG_HOME.
    pub fn retrieve(config_path: Option<PathBuf>) -> Result<Self> {
        Ok(Self {
            bliss: Library::from_config_path(config_path).context("while retrieving bliss library")?,
            mpd_conn: Arc::new(Mutex::new(Self::connect_to_mpd().context("while connecting to MPD")?)),
            genre_weights: None,
        })
    }

    // Gets all songs from MPD. May fail if the MPD connection is dropped.
    pub fn get_all_mpd_songs(&self) -> Result<Vec<String>> {
        let mut songs = vec![];
        let mut query = Query::new();
        let query = query.and(Term::File, "");
        let (mut index, chunk_size) = (0, 10000);
        loop {
            let search = self
                .mpd_conn
                .lock()
                .expect("Poisoned lock")
                .search(query, Window::from((index, index + chunk_size)))?;
            if search.is_empty() {
                break;
            }
            songs.extend(search.into_iter().map(|s| s.file.to_owned()).map(|s| {
                String::from(
                    Path::new(&self.bliss.config.mpd_base_path)
                        .join(Path::new(&s))
                        .to_str()
                        .expect(format!("Song path not valid Unicode: {s}").as_str()),
                )
            }));
            index += chunk_size;
            songs.sort();
            songs.dedup();
        }
        Ok(songs)
    }

    /// Analyzes all songs in MPD's database with bliss. May fail if the database connection is dropped,
    /// if the MPD connection is dropped, if the database is corrupted, or if analysis fails. Analysis will
    /// typically continue even if individual songs fail.
    pub fn populate(&mut self) -> Result<()> {
        let sqlite_conn = self.bliss.sqlite_conn.lock().expect("Poisoned lock");
        let mut songs_query = sqlite_conn.prepare("select * from song").context("while preparing bliss database query")?;
        let songs_result = songs_query.query([]).context("while querying bliss database")?;
        let songs_total = songs_result.count().context("while counting bliss database query results")?;
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
        self.bliss.analyze_paths(self.get_all_mpd_songs().context("while getting all MPD songs")?, true)
    }

    /// Gets the bliss path of an [mpd::song::Song] by prepending the MPD base path.
    fn mpd_to_bliss_path(&self, mpd_song: &MPDSong) -> Result<PathBuf> {
        let file = &mpd_song.file;
        let path = file.to_string();
        let path = &self.bliss.config.mpd_base_path.join(PathBuf::from(&path));
        Ok(path.to_path_buf())
    }

    /// Converts an [mpd::song::Song] to a [bliss_audio::library::LibrarySong], if previously analyzed.
    pub fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<BlissSong<()>>> {
        let path = self.mpd_to_bliss_path(mpd_song).context("while converting MPD path to bliss path")?;
        let song = self.bliss.song_from_path(&path.to_string_lossy()).ok();
        Ok(song)
    }

    /// Converts a [bliss_audio::library::LibrarySong] to an [mpd::song::Song].
    fn bliss_song_to_mpd(&self, song: &BlissSong<()>) -> Result<MPDSong> {
        let path = song.bliss_song.path.to_owned();
        let path = path.strip_prefix(&*self.bliss.config.mpd_base_path.to_string_lossy()).context("while stripping prefix from bliss path")?;
        Ok(MPDSong {
            file: path.to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    /// The meat of `worf`. Continuously queues songs from the MPD library based on similarity to the song passed as
    /// argument until it reaches the end of the user's library. The distance metric can be customized, as well as
    /// the sort function. May fail if the database connection is dropped, if bliss fails to create a playlist,
    /// if the song passed in has not been analyzed, or if MPD gets confused when the user makes a lot of changes
    /// to the queue in quick succession (working on ways around this).
    pub fn queue_from_song<'a, F, I>(
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
        let mut mpd_conn = self.mpd_conn.lock().expect("Poisoned lock");
        let path = self.bliss.config.mpd_base_path.join(&song.file);
        let mut playlist = self
            .bliss
            .playlist_from_custom(&[&path.to_string_lossy().clone()], distance, sort_by, dedup).context("while building bliss playlist")?
            .skip(1);

        let mut last_status = mpd_conn.status().context("while getting MPD status")?;
        let current_pos = song
            .place
            .ok_or(anyhow!("Failed to get current song position"))?
            .pos;
        if !keep_queue {
            mpd_conn.delete(0..current_pos).context("while deleting songs from MPD queue")?;
            if mpd_conn.queue().context("while getting MPD queue")?.len() > 1 {
                mpd_conn.delete(1..).context("while deleting songs from MPD queue")?;
            }
        }

        for _ in 0..num_songs {
            let mpd_song = self.bliss_song_to_mpd(
                playlist
                    .next()
                    .as_ref()
                    .ok_or(anyhow!("Failed to get next song from bliss"))?,
            ).context("while converting bliss path to MPD path")?;
            mpd_conn.push(mpd_song).context("while pushing songs to MPD queue")?;
        }

        for bliss_song in playlist {
            let next_event = mpd_conn.wait(&[mpd::Subsystem::Queue]).context("while waiting on events from MPD")?;

            if next_event[0] == mpd::Subsystem::Queue {
                let new_status = mpd_conn.status().context("while getting MPD status")?;
                let last_song = last_status
                    .song
                    .ok_or(anyhow!("Failed to get last song position"))?;
                let new_song = new_status
                    .song
                    .ok_or(anyhow!("Failed to get current song position"))?;
                // TODO: change to start queueing when reaching the end
                if new_song.pos == last_song.pos + 1 {
                    let mpd_song = self.bliss_song_to_mpd(&bliss_song).context("while converting bliss path to MPD path")?;
                    mpd_conn.push(mpd_song).context("while pushing songs to MPD queue")?;
                    last_status = new_status;
                } else if new_song.id == last_song.id {
                    last_status = new_status;
                    continue;
                } else if new_status.state == mpd::status::State::Play {
                    return Ok(mpd_conn
                        .currentsong().context("while getting current song from MPD")?
                        .ok_or(anyhow!("Failed to get current song"))?);
                }
            }
        }

        Ok(song.clone())
    }

    /// Loads genre weights from disk and associates them with tracks in the bliss library. May fail if the weights are not found,
    /// if they're in the wrong format, or if the bliss library is corrupted.
    pub fn get_track_genre_weights(&mut self) -> Result<TrackWeights> {
        let all_bliss_songs = self.bliss.songs_from_library::<()>().context("while getting bliss library")?;

        let genre_weights: GenreWeights =
            serde_json::from_reader(File::open("./genres.json").context("while opening genres.json")?)
                .context("while parsing genres.json")?;
        self.genre_weights = Some(genre_weights.clone());

        let mut genre_weights_by_track_path: TrackWeights = HashMap::new();
        for song in all_bliss_songs {
            if let Some(genres) = song.bliss_song.genre {
                genre_weights_by_track_path
                    .entry(
                        song.bliss_song
                            .path
                            .to_str()
                            .ok_or(anyhow!(format!(
                                "Song path not valid Unicode: {:?}",
                                song.bliss_song.path
                            )))?
                            .to_owned(),
                    )
                    .or_insert(collapse_genres(&genre_weights, genres));
            }
        }
        Ok(genre_weights_by_track_path)
    }
}
