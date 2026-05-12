mod ffmpeg_decoder;

use crate::NUM_GENRE_FEATURES;
use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::{
    AnalysisOptions, Song as BareBlissSong,
    library::{AppConfigTrait, BaseConfig, Library, LibrarySong as BlissSongNoInfo},
    playlist::{DistanceMetricBuilder, euclidean_distance},
};
use fallible_streaming_iterator::FallibleStreamingIterator;
use ffmpeg_decoder::FFmpegDecoder as Decoder;
use itertools::Itertools;
use log::{debug, info};
use mpd::{Client, Idle, Query, Song as MPDSong, Term, search::Window};
use ndarray::{Array1, arr1};
use noisy_float::prelude::n32;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::TcpStream;
#[cfg(target_os = "android")]
use std::os::android::net::SocketAddrExt;
#[cfg(target_os = "linux")]
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, time::Instant};
use std::{env, sync::MutexGuard};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExtraInfo {
    pub popularity: i32,
}

pub type BlissSong = BlissSongNoInfo<ExtraInfo>;

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
    pub mpd_base_path: PathBuf,
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

pub fn collapse_genres<const N: usize>(genre_weights: &GenreWeights, genres: String) -> Vec<f32> {
    if genres.is_empty() {
        vec![0.0; N]
    } else {
        vec_avg::<N>(
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
    assert!(
        !track_weights.is_empty(),
        "Likely tried to call genre_sort under `daemon`"
    );
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
/// https://github.com/rust-lang/rust/issues/133508
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

type GenreName = String;
/// A mapping of genre names to a 5-tuple of weights along the axes from everynoise.com: (organicness/mechanicity, etherealness/spikiness, energy, dynamic variation, instrumentalness). (see https://www.furia.com/page.cgi?type=log&id=419 for the last three values).
/// (The 5-tuple is a Vec for now until variadics make it into the language, because I want to keep support for genre weights with different numbers of features.)
pub type GenreWeights = HashMap<GenreName, Vec<f32>>;

/// A mapping of track names to calculated genre weights from a Eucliean average of their genre names. Might make sense to customize the averaging function in the future.
type TrackWeights = HashMap<String, Vec<f32>>;

/// The main struct which holds the [bliss_audio::library::Library] and [mpd::Client]. Also holds the [GenreWeights] if present.
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
        let config = Config::build(mpd_base_path.clone(), config_path, database_path, None)
            .context("while building bliss Config")?;
        Ok(Self {
            bliss: Library::new(config).context("while building bliss library")?,
            mpd_conn: Arc::new(Mutex::new(
                Self::connect_to_mpd().context("while connecting to MPD")?,
            )),
            genre_weights: None,
        })
    }

    fn maybe_retrieve(config_path: Option<PathBuf>) -> Result<Self> {
        let bliss_library: Library<Config, Decoder> =
            Library::from_config_path(config_path).context("while retrieving bliss library")?;
        Ok(Self {
            bliss: bliss_library,
            mpd_conn: Arc::new(Mutex::new(
                Self::connect_to_mpd().context("while connecting to MPD")?,
            )),
            genre_weights: None,
        })
    }

    /// Retrieve an existing MPDLibrary from disk. May panic if path provided doesn't exist or if an error occurs
    /// connecting to MPD. If no path is provided, bliss will look up a configuration in $XDG_CONFIG_HOME.
    pub fn retrieve(config_path: Option<PathBuf>) -> Result<Self> {
        let maybe_library = Self::maybe_retrieve(config_path);

        match maybe_library {
            Ok(library) => Ok(library),
            Err(e) => match e.downcast::<std::io::Error>() {
                Ok(inner) => match inner.kind() {
                    std::io::ErrorKind::NotFound => {
                        Err(anyhow!("Must initialize library first using `worf init`"))
                    }
                    kind => Err(anyhow!(
                        "Could not retrieve or create MPDLibrary, io error: {kind}"
                    )),
                },
                Err(inner_err) => Err(anyhow!(
                    "Could not retrieve or create MPDLibrary (non-io error: {inner_err})"
                )),
            },
        }
    }

    /// Gets all songs from MPD as [MPDSong]. May fail if MPD connection is dropped.
    fn get_all_mpd_songs_full(&self) -> Result<Vec<MPDSong>> {
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
            songs.extend(search);
            index += chunk_size;
            songs.dedup();
        }
        Ok(songs)
    }

    pub fn get_songs_extra_info(&self) -> Result<Vec<(String, ExtraInfo)>> {
        let all_songs = self.get_all_mpd_songs_full()?;
        let mut all_songs_extra_info = vec![];
        let mut default_pop = 0;
        for song in all_songs {
            all_songs_extra_info.push((
                String::from(
                    Path::new(&self.bliss.config.mpd_base_path)
                        .join(Path::new(&song.file))
                        .to_str()
                        .unwrap_or_else(|| panic!("Song path not valid Unicode: {}", song.file)),
                ),
                ExtraInfo {
                    popularity: song
                        .tags
                        .iter()
                        .find(|(tag_name, _)| tag_name.eq_ignore_ascii_case("comment"))
                        .map(|(_, pop_val)| {
                            serde_json::from_str::<ExtraInfo>(pop_val)
                                .expect("while parsing comment tag as json")
                                .popularity
                        })
                        .unwrap_or_else(|| {
                            default_pop += 1;
                            0
                        }),
                },
            ))
        }
        Ok(all_songs_extra_info)
    }

    // Updates bliss database with new songs from MPD. May fail if the database connection is dropped, if the MPD connection is dropped, if the database is corrupted, or if analysis fails. Analysis will typically continue even if individual songs fail.
    pub fn update(&mut self) -> Result<()> {
        self.bliss
            .update_library_extra_info(self.get_songs_extra_info()?, true, true)
    }

    /// Analyzes all songs in MPD's database with bliss. May fail if the database connection is dropped,
    /// if the MPD connection is dropped, if the database is corrupted, or if analysis fails. Analysis will
    /// typically continue even if individual songs fail.
    pub fn populate(&mut self) -> Result<()> {
        let sqlite_conn = self.bliss.sqlite_conn.lock().expect("Poisoned lock");
        let mut songs_query = sqlite_conn
            .prepare("select * from song")
            .context("while preparing bliss database query")?;
        let songs_result = songs_query
            .query([])
            .context("while querying bliss database")?;
        let songs_total = songs_result
            .count()
            .context("while counting bliss database query results")?;
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
        self.bliss.analyze_paths_extra_info(
            self.get_songs_extra_info()?,
            true,
            AnalysisOptions::default(),
        )
    }

    /// Gets the bliss path of an [mpd::song::Song] by prepending the MPD base path.
    fn mpd_to_bliss_path(&self, mpd_song: &MPDSong) -> Result<PathBuf> {
        let file = &mpd_song.file;
        let path = file.to_string();
        let path = self.bliss.config.mpd_base_path.join(PathBuf::from(&path));
        Ok(path)
    }

    /// Converts an [mpd::song::Song] to a [bliss_audio::library::LibrarySong], if previously analyzed.
    pub fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<BlissSong>> {
        let path = self
            .mpd_to_bliss_path(mpd_song)
            .context("while converting MPD path to bliss path")?;
        let song = self.bliss.song_from_path(&path.to_string_lossy()).ok();
        Ok(song)
    }

    /// Finds the [bliss_audio::library::LibrarySong] matching a filename, if previously analyzed.
    fn path_to_bliss_song(&self, filename: &str) -> Result<BlissSong> {
        self.bliss.song_from_path(
            &self
                .bliss
                .config
                .mpd_base_path
                .join(PathBuf::from(filename))
                .to_string_lossy(),
        )
    }

    /// Converts a [bliss_audio::library::LibrarySong] to an [mpd::song::Song].
    fn bliss_song_to_mpd(&self, song: &BlissSong) -> Result<MPDSong> {
        let path = song.bliss_song.path.to_owned();
        let path = path
            .strip_prefix(&*self.bliss.config.mpd_base_path.to_string_lossy())
            .context("while stripping prefix from bliss path")?;
        Ok(MPDSong {
            file: path.to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    /// Retrieves the currently playing song, or waits for one to become available. Needs exclusive access to the [MPDLibrary::mpd_conn].
    pub fn get_current_song(&self) -> Result<MPDSong> {
        let current_song = self.mpd_conn.lock().expect("Poisoned lock").currentsong();
        match current_song {
            Ok(song) => {
                if let Some(song) = song {
                    Ok(song)
                } else {
                    println!("Start playing a song...");
                    loop {
                        let next_event = self
                            .mpd_conn
                            .lock()
                            .expect("Poisoned lock")
                            .wait(&[mpd::Subsystem::Queue])
                            .context("while waiting on events from MPD")?;
                        if !next_event.is_empty() && next_event[0] == mpd::Subsystem::Queue {
                            return self.get_current_song();
                        }
                    }
                }
            }
            Err(e) => Err(e)
                .context("while getting current song from MPD")
                .map_err(|e| anyhow!(e)),
        }
    }

    fn get_bliss_similarity(&self, next_song: &BlissSong, original_song: &BlissSong) -> f32 {
        100.0
            - euclidean_distance(
                &original_song.bliss_song.analysis.as_arr1(),
                &next_song.bliss_song.analysis.as_arr1(),
            ) * 100.0
    }

    fn get_genre_similarity(
        &self,
        next_song: &BlissSong,
        original_song: &BlissSong,
    ) -> Result<f32> {
        let genre_weights = self.genre_weights.as_ref().ok_or(anyhow!(
            "Genre weights not found, likely forgot to call `get_track_genre_weights`"
        ))?;
        Ok(100.0
            - euclidean_distance(
                &arr1(
                    slice_as_array::<NUM_GENRE_FEATURES, f32>(
                        &collapse_genres::<NUM_GENRE_FEATURES>(
                            genre_weights,
                            original_song
                                .bliss_song
                                .genre
                                .clone()
                                .context("while getting original song genre")?,
                        ),
                    )
                    .context("while converting genre weight slice to array")?,
                ),
                &arr1(
                    slice_as_array::<NUM_GENRE_FEATURES, f32>(
                        &collapse_genres::<NUM_GENRE_FEATURES>(
                            genre_weights,
                            next_song
                                .bliss_song
                                .genre
                                .clone()
                                .context("while getting next song genre")?,
                        ),
                    )
                    .context("while converting genre weight slice to array")?,
                ),
            ) * 100.0)
    }

    fn add_next_song_from_playlist<P>(
        &self,
        playlist: &mut P,
        mpd_conn: &mut MutexGuard<Client<MPDStream>>,
        history: &mut Vec<String>,
        distance: u32,
        original_song: &BlissSong,
    ) -> Result<()>
    where
        P: Iterator<Item = BlissSong>,
    {
        let next_song_object = playlist.next();
        let next_song = next_song_object
            .as_ref()
            .ok_or(anyhow!("while getting next song from bliss"))?;
        let mut mpd_song = self
            .bliss_song_to_mpd(next_song)
            .context("while converting bliss path to MPD path")?;
        history.push(mpd_song.file.clone());
        info!(
            "Bliss similarity of next song ({}) to pin ({}): {:.2}%",
            next_song
                .bliss_song
                .title
                .as_ref()
                .unwrap_or(&"Unknown".into()),
            original_song
                .bliss_song
                .title
                .as_ref()
                .unwrap_or(&"Unknown".into()),
            self.get_bliss_similarity(next_song, original_song)
        );
        info!(
            "Next song genres: {}",
            next_song
                .bliss_song
                .genre
                .as_ref()
                .unwrap_or(&"Unknown".into())
        );
        info!(
            "Genre similarity: {:?}",
            self.get_genre_similarity(next_song, original_song)
        );
        info!("Popularity: {}", next_song.extra_info.popularity);
        let title = mpd_song.title.take();
        let result = mpd_conn.push(mpd_song);
        if let Err(e) = result {
            println!(
                "Error while pushing song {} to MPD queue, skipping: {e}",
                title.clone().unwrap_or("Unknown".to_string())
            );
        }
        debug!(
            "Queued song {}, distance = {}",
            title.unwrap_or("Unknown".to_string()),
            distance
        );
        Ok(())
    }

    /*
    pub fn save_playlist<'a, F, I>(
        &self,
        song: &MPDSong,
        playlist_length: u32,
        distance: &'a dyn DistanceMetricBuilder,
        sort_by: F,
        dedup: bool,
    ) -> Result<()>
    where
        F: Fn(&[BlissSong], &[BlissSong], &'a dyn DistanceMetricBuilder) -> I,
        I: Iterator<Item = BlissSong> + 'a,
    {
        let mut mpd_conn = self.mpd_conn.lock().expect("Poisoned lock");
        let path = self.bliss.config.mpd_base_path.join(&song.file);
        let mut playlist = self
            .bliss
            .playlist_from_custom(&[&path.to_string_lossy().clone()], distance, sort_by, dedup)
            .context("while building bliss playlist")?
            .skip(1)
            .take(playlist_length as usize);

        // TODO:
        // 1) add playlist to MPD
        // 2) come up with a way to uniquely identify playlists. do they have any metadata fields? if not,
        // can a hash based on the initial song be integrated into the playlist name somehow?
        // 3) refuse to generate a playlist if it already exists, only update the existing one
        // 4) wait for a specified time and update again, forever
        // 5) instead of waiting forever, branch off a thread each time this function is called with a different song
        // to allow for multiple watches
        // 6) option to allow for shuffling the playlist within some initial window (whether by # of songs or by similarity %)
        Ok(())
    }
    */

    /// Continuously queues songs from the MPD library based on similarity to the song passed as
    /// argument until it reaches the end of the user's library. `queue_length` determines how
    /// many recommendations will be queued up at a time. The distance metric can be customized,
    /// as well as the sort function. Use `keep_queue` to set the pin whenever a new song(s)
    /// is queued, but don't immediately overwrite the queue -- useful for queueing playlists and
    /// generating recommendations at the end. May fail if the database connection is dropped, if bliss
    /// fails to create a playlist, or if the song passed in has not been analyzed.
    pub fn queue_from_song<'a, F, G, I>(
        &self,
        song: &MPDSong,
        queue_length: u32,
        distance: &'a dyn DistanceMetricBuilder,
        sort_by: F,
        mut filter_by: Option<G>,
        dedup: bool,
        keep_queue: bool,
        timestamp: Instant,
    ) -> Result<MPDSong>
    where
        F: Fn(&[BlissSong], &[BlissSong], &'a dyn DistanceMetricBuilder) -> I,
        G: for<'b> FnMut(&'b BlissSong, &'b BlissSong) -> bool,
        I: Iterator<Item = BlissSong> + 'a,
    {
        let mut mpd_conn = self.mpd_conn.lock().expect("Poisoned lock");
        let path = self.bliss.config.mpd_base_path.join(&song.file);
        let bliss_song = self.path_to_bliss_song(&song.file)?;
        info!("Pin popularity: {}", bliss_song.extra_info.popularity);
        let filter = |s: &BlissSong| {
            if let Some(ref mut filter_fn) = filter_by {
                filter_fn(s, &bliss_song)
            } else {
                true
            }
        };
        let mut playlist = self
            .bliss
            .playlist_from_custom(&[&path.to_string_lossy().clone()], distance, sort_by, dedup)
            .context("while building bliss playlist")?
            .filter(filter)
            .skip(1);

        let current_pos = song
            .place
            .ok_or(anyhow!("while getting initial current song position"))?
            .pos;
        if !keep_queue {
            mpd_conn
                .delete(0..current_pos)
                .context("while deleting songs from MPD queue")?;
            if mpd_conn.queue().context("while getting MPD queue")?.len() > 1 {
                mpd_conn
                    .delete(1..)
                    .context("while deleting songs from MPD queue")?;
            }
        }

        let mut history: Vec<String> = vec![];

        let status = mpd_conn.status()?;

        let queue_pos = status
            .song
            .ok_or(anyhow!("while getting current song position"))?
            .pos;

        let mut queue_diff = queue_length.saturating_sub(status.queue_len - queue_pos + 1);

        if (status.queue_len <= queue_length) || (queue_length > queue_diff) {
            while queue_diff > 0 {
                self.add_next_song_from_playlist(
                    &mut playlist,
                    &mut mpd_conn,
                    &mut history,
                    0,
                    &bliss_song,
                )?;
                queue_diff -= 1;
            }
        }

        info!(
            "Time to first recommendations: {}ms",
            timestamp.elapsed().as_millis()
        );

        let mut last_queue = mpd_conn.queue()?;

        loop {
            let next_event = mpd_conn
                .wait(&[mpd::Subsystem::Queue])
                .context("while waiting on events from MPD")?;

            if !next_event.is_empty() && next_event[0] == mpd::Subsystem::Queue {
                let status = mpd_conn.status()?;
                let new_queue = mpd_conn.queue()?;
                if new_queue.len() != last_queue.len() {
                    // don't restart if the new queue is the old queue plus any of the songs from the generated playlist, otherwise use the currently playing song as the new pin
                    let last_queue_songs: Vec<&MPDSong> = last_queue.iter().collect();
                    let all_songs_same_or_generated = new_queue.iter().all(|song| {
                        last_queue_songs.contains(&song) || history.contains(&song.file)
                    });
                    if !all_songs_same_or_generated {
                        let new_pin = mpd_conn
                            .currentsong()
                            .context("while getting current song from MPD")?
                            .ok_or(anyhow!("while getting current song from MPD"))?;
                        println!(
                            "Restarting with new pin: {}",
                            new_pin
                                .title
                                .as_ref()
                                .ok_or(anyhow!("while getting pin title"))?
                        );
                        return Ok(new_pin);
                    }
                } else {
                    // don't restart if new queue is just a reshuffling of the old queue, but do restart if any songs are different. assume the now playing song is the new pin
                    let new_queue_sorted = new_queue
                        .iter()
                        .sorted_by(|a, b| Ord::cmp(&a.file, &b.file));
                    let last_queue_sorted = last_queue
                        .iter()
                        .sorted_by(|a, b| Ord::cmp(&a.file, &b.file));
                    let all_songs_same = new_queue_sorted
                        .zip(last_queue_sorted)
                        .all(|(new, old)| new.file == old.file);
                    if !all_songs_same {
                        let new_pin = mpd_conn
                            .currentsong()
                            .context("while getting current song from MPD")?
                            .ok_or(anyhow!("while getting current song from MPD"))?;
                        println!(
                            "Restarting with new pin: {}",
                            new_pin
                                .title
                                .as_ref()
                                .ok_or(anyhow!("while getting pin title"))?
                        );
                        return Ok(new_pin);
                    }
                }

                last_queue = new_queue;

                let queue_pos = status
                    .song
                    .ok_or(anyhow!("while getting current song queue position"))?
                    .pos;

                if status.queue_len <= queue_length || queue_pos >= status.queue_len - queue_length
                {
                    self.add_next_song_from_playlist(
                        &mut playlist,
                        &mut mpd_conn,
                        &mut history,
                        // sort_by,
                        0,
                        &bliss_song,
                    )?;
                }
            }
        }
    }

    /// Loads genre weights from disk and associates them with tracks in the bliss library. May fail if the weights are not found,
    /// if they're in the wrong format, or if the bliss library is corrupted.
    pub fn get_track_genre_weights<const N: usize>(
        &mut self,
        genres_path: Option<PathBuf>,
    ) -> Result<TrackWeights> {
        let all_bliss_songs: Vec<BlissSong> = self
            .bliss
            .songs_from_library()
            .context("while getting bliss library")?;

        let genre_weights: GenreWeights = serde_json::from_reader(
            File::open(genres_path.unwrap_or("./genres.json".into()))
                .context("while opening genre weights json")?,
        )
        .context("while parsing genre weights json")?;
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
                    .or_insert(collapse_genres::<N>(&genre_weights, genres));
            }
        }
        Ok(genre_weights_by_track_path)
    }
}
