mod ffmpeg_decoder;

use crate::{NUM_BLISS_FEATURES, NUM_GENRE_FEATURES};
use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::{
    AnalysisOptions, Song as BareBlissSong,
    library::{AppConfigTrait, BaseConfig, Library, LibrarySong as BlissSongNoInfo},
    playlist::{DistanceMetricBuilder, euclidean_distance},
};
use fallible_streaming_iterator::FallibleStreamingIterator;
use ffmpeg_decoder::FFmpegDecoder as Decoder;
use itertools::Itertools;
use log::{debug, info, warn};
use mpd::{Client, Idle, Query, Song as MPDSong, Term, search::Window};
use ndarray::{Array1, arr1};
use noisy_float::prelude::n32;
use rocket::tokio::sync::{Mutex, MutexGuard};
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
use std::sync::{Arc, atomic::Ordering};
use std::{collections::HashMap, time::Instant};
use std::{env, sync::atomic::AtomicBool};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

fn genre_vec_avg(v: Vec<[f32; NUM_GENRE_FEATURES]>) -> Vec<f32> {
    v.iter().map(|arr| average(&arr[..])).collect()
}

pub fn pad_slice<const N: usize>(v: &[f32]) -> [f32; N] {
    assert!(
        v.len() <= N,
        "length of slice to pad {} must be less than final length {}",
        v.len(),
        N
    );
    let mut result = [0.0; N];
    result[..v.len()].copy_from_slice(v);
    result
}

pub fn collapse_genres_pad_to<const N: usize>(
    genre_weights: &GenreWeights,
    genres: String,
) -> [f32; N] {
    assert!(
        !genre_weights.is_empty(),
        "Likely tried to call collapse_genres under `bliss`"
    );
    assert!(
        N >= NUM_GENRE_FEATURES,
        "Cannot pad to a length less than number of genre features"
    );
    if genres.is_empty() {
        [0.0; N]
    } else {
        let genres_vec: Vec<_> = genres.split(",").collect();
        let genres_vec: Vec<_> = genres_vec
            .into_iter()
            .filter_map(|genre| genre_weights.get(genre))
            .map(|weights| weights.to_owned())
            .collect();
        let result = if genres_vec.len() as f32 * NUM_GENRE_FEATURES as f32 <= N as f32 {
            // use full genre weights
            genres_vec.into_iter().flatten().collect()
        } else {
            // collapse each genre into one feature
            genre_vec_avg(genres_vec)
        };
        if result.len() > N {
            info!(
                "Clipping genre features with length {} to fit into array of length {}",
                result.len(),
                N
            );
            result[..N].try_into().unwrap_or([0.0; N])
        } else {
            pad_slice(&result)
        }
    }
}

pub fn collapse_genres(genre_weights: &GenreWeights, genres: String) -> [f32; NUM_BLISS_FEATURES] {
    collapse_genres_pad_to(genre_weights, genres)
}

/// A modified version of the bliss default playlist creator, sorting by genre weights and using bliss similarity as a tiebreaker.
pub fn closest_to_genre_songs<'a, T: AsRef<BareBlissSong> + Clone + 'a>(
    initial_songs: &[T],
    candidate_songs: &[T],
    metric_builder: &'a dyn DistanceMetricBuilder,
    track_weights: &TrackWeights,
) -> impl Iterator<Item = T> + 'a {
    assert!(
        !track_weights.is_empty(),
        "Likely tried to call genre_sort under `bliss`"
    );
    let initial_songs_bliss_weights: Vec<Array1<f32>> = initial_songs
        .iter()
        .map(|c| c.as_ref().analysis.as_arr1())
        .collect();
    let bliss_metric = metric_builder.build(&initial_songs_bliss_weights);
    let initial_songs_genre_weights: Vec<Array1<f32>> = initial_songs
        .iter()
        .filter_map(|c| Some(arr1(track_weights.get(&*c.as_ref().path)?)))
        .collect();
    let genre_metric = metric_builder.build(&initial_songs_genre_weights);
    let mut candidate_songs = candidate_songs.to_vec();
    candidate_songs.sort_by_cached_key(|song| {
        (
            n32(genre_metric.distance(&arr1(
                track_weights
                    .get(&*song.as_ref().path)
                    .unwrap_or(&[0.0; NUM_BLISS_FEATURES]),
            ))),
            n32(bliss_metric.distance(&song.as_ref().analysis.as_arr1())),
        )
    });
    candidate_songs.into_iter()
}

type GenreName = String;
/// A mapping of genre names to an array of weights along the axes from everynoise.com: (organicness/mechanicity, etherealness/spikiness, energy, dynamic variation, instrumentalness). (see https://www.furia.com/page.cgi?type=log&id=419 for the last three values).
pub type GenreWeights = HashMap<GenreName, [f32; NUM_GENRE_FEATURES]>;

type TrackPath = PathBuf;
/// A mapping of track names to calculated genre weights from a Eucliean average of their genre names. Might make sense to customize the averaging function in the future.
type TrackWeights = HashMap<TrackPath, [f32; NUM_BLISS_FEATURES]>;

/// The main struct which holds the bliss library and MPD connection. Also holds the genre weights if present.
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

    pub fn reconnect_to_mpd(mpd_conn: &mut MutexGuard<Client<MPDStream>>) {
        let mut counter = 1;
        loop {
            let result = Self::connect_to_mpd();
            match result {
                Ok(new_conn) => {
                    **mpd_conn = new_conn;
                    println!("Reconnected to MPD!");
                    break;
                }
                Err(_) => {
                    let backoff = 2_u64.pow(counter);
                    println!("Reconnecting in {backoff} seconds...");
                    std::thread::sleep(std::time::Duration::from_secs(backoff));
                    counter += 1;
                }
            }
        }
    }

    /// Build a new MPDLibrary.
    ///
    /// May fail if paths provided don't exist or if an error occurs connecting to MPD.
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

    /// Retrieve an existing MPDLibrary from disk.
    ///
    /// May panic if path provided doesn't exist or if an error occurs
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

    /// Get all songs from MPD as [MPDSong]. May fail if MPD connection is dropped.
    async fn get_all_mpd_songs_full(&self) -> Result<Vec<MPDSong>> {
        let mut songs = vec![];
        let mut query = Query::new();
        let query = query.and(Term::File, "");
        let (mut index, chunk_size) = (0, 10000);
        loop {
            let search = self
                .mpd_conn
                .lock()
                .await
                // .expect("Poisoned lock")
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

    /// Get extra info for all songs.
    ///
    /// May fail if MPD connection is dropped.
    pub async fn get_songs_extra_info(&self) -> Result<Vec<(String, ExtraInfo)>> {
        let all_songs = self.get_all_mpd_songs_full().await?;
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
                            serde_json::from_str(pop_val)
                                .unwrap_or_else(|_| { warn!("Couldn't parse comment tag as json at {}, tag contents: {pop_val}, using default", song.file); ExtraInfo::default() })
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

    /// Update bliss database with new songs from MPD.
    ///
    /// May fail if the database connection is dropped, if the MPD connection is dropped, if the database is corrupted, or if analysis fails. Analysis will typically continue even if individual songs fail.
    pub async fn update(&mut self) -> Result<()> {
        println!("Updating library...");
        self.bliss
            .update_library_extra_info(self.get_songs_extra_info().await?, true, true)
    }

    /// Analyze all songs in MPD's database with bliss.
    ///
    /// May fail if the database connection is dropped,
    /// if the MPD connection is dropped, if the database is corrupted, or if analysis fails. Analysis will
    /// typically continue even if individual songs fail.
    pub async fn populate(&mut self) -> Result<()> {
        {
            // this block needed to not hold sqlite_conn across an `await`
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
                    println!(
                        "Database contains data ({songs_total} songs). Continue anyway? (Y/N)"
                    );
                    let mut answer = String::new();
                    io::stdin()
                        .read_line(&mut answer)
                        .expect("Failed to read answer");
                    let answer: char = match answer.trim().to_uppercase().parse() {
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
        }
        self.bliss.analyze_paths_extra_info(
            self.get_songs_extra_info().await?,
            true,
            AnalysisOptions::default(),
        )
    }

    /// Get the bliss path of an MPD song by prepending the MPD base path.
    fn mpd_to_bliss_path(&self, mpd_song: &MPDSong) -> Result<PathBuf> {
        let file = &mpd_song.file;
        let path = file.to_string();
        let path = self.bliss.config.mpd_base_path.join(PathBuf::from(&path));
        Ok(path)
    }

    /// Convert an MPD song to a bliss song, if previously analyzed.
    pub fn mpd_to_bliss_song(&self, mpd_song: &MPDSong) -> Result<Option<BlissSong>> {
        let path = self
            .mpd_to_bliss_path(mpd_song)
            .context("while converting MPD path to bliss path")?;
        let song = self.bliss.song_from_path(&path.to_string_lossy()).ok();
        Ok(song)
    }

    /// Find the bliss song matching a filename, if previously analyzed.
    pub fn path_to_bliss_song(&self, filename: &str) -> Result<BlissSong> {
        self.bliss.song_from_path(
            &self
                .bliss
                .config
                .mpd_base_path
                .join(PathBuf::from(filename))
                .to_string_lossy(),
        )
    }

    /// Convert a bliss song to an MPD song.
    pub fn bliss_song_to_mpd(&self, song: &BlissSong) -> Result<MPDSong> {
        let path = song.bliss_song.path.to_owned();
        let path = path
            .strip_prefix(&*self.bliss.config.mpd_base_path.to_string_lossy())
            .context("while stripping prefix from bliss path")?;
        Ok(MPDSong {
            file: path.to_string_lossy().to_string(),
            ..Default::default()
        })
    }

    /// Retrieve the currently playing song, or wait for one to become available. Needs exclusive access to the MPD connection.
    pub async fn get_current_song(&self) -> Result<MPDSong> {
        let current_song = self.mpd_conn.lock().await.currentsong();
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
                            .await
                            .wait(&[mpd::Subsystem::Queue])
                            .context("while waiting on events from MPD")?;
                        if !next_event.is_empty() && next_event[0] == mpd::Subsystem::Queue {
                            return Box::pin(self.get_current_song()).await;
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
        debug!(
            "Original song features: {:?}",
            original_song.bliss_song.analysis.as_arr1()
        );
        debug!(
            "Next song features: {:?}",
            next_song.bliss_song.analysis.as_arr1()
        );
        euclidean_distance(
            &original_song.bliss_song.analysis.as_arr1(),
            &next_song.bliss_song.analysis.as_arr1(),
        )
    }

    fn get_genre_similarity(
        &self,
        next_song: &BlissSong,
        original_song: &BlissSong,
    ) -> Result<f32> {
        let original_genre_weights = self.genre_weights.as_ref().ok_or(anyhow!(
            "Genre weights not found, likely forgot to call `get_track_genre_weights`"
        ))?;
        Ok(100.0
            - euclidean_distance(
                &arr1(&collapse_genres(
                    original_genre_weights,
                    original_song
                        .bliss_song
                        .genre
                        .clone()
                        .context("while getting original song genre")?,
                )),
                &arr1(&collapse_genres(
                    original_genre_weights,
                    next_song
                        .bliss_song
                        .genre
                        .clone()
                        .context("while getting next song genre")?,
                )),
            ) * 100.0)
    }

    fn add_next_song_from_playlist<P>(
        &self,
        playlist: &mut P,
        mpd_conn: &mut MutexGuard<Client<MPDStream>>,
        history: &mut Vec<String>,
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
            "Bliss distance from next song ({}) to pin ({}): {:.2} units",
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
        if let Ok(genre_similarity) = self.get_genre_similarity(next_song, original_song) {
            info!("Genre similarity: {:?}", genre_similarity);
        }
        info!("Popularity: {}", next_song.extra_info.popularity);
        let title = mpd_song.title.take();
        let filename = mpd_song.file.clone();
        let result = mpd_conn.push(mpd_song);
        if let Err(e) = result {
            println!(
                "Error while pushing song {} to MPD queue, skipping: {e}",
                title.clone().unwrap_or("Unknown".to_string())
            );
        }
        debug!("Queued song {}", title.unwrap_or(filename),);
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

    /// Continuously queue songs from the MPD library based on similarity to the song passed as
    /// argument until it reaches the end of the user's library. `queue_length` determines how
    /// many recommendations will be queued up at a time. The distance metric can be customized,
    /// as well as the sort function. A filter function can optionally be provided. Use `keep_queue`
    /// to set the pin whenever a new song(s) is queued without immediately overwriting the queue --
    /// useful for queueing playlists and generating recommendations at the end.
    ///
    /// May fail if the database connection is dropped, if bliss fails to create a playlist, or if
    /// the song passed in has not been analyzed.
    #[allow(clippy::too_many_arguments)]
    pub async fn queue_from_song<'a, F, G>(
        &mut self,
        song: &MPDSong,
        queue_length: u32,
        distance: &'a (dyn DistanceMetricBuilder + Sync),
        sort_by: F,
        mut filter_by: Option<G>,
        dedup: bool,
        keep_queue: bool,
        timestamp: Instant,
        update_on_next_loop: Arc<AtomicBool>,
    ) -> Result<MPDSong>
    where
        F: for<'c, 'd, 'e> Fn(
            &'c [BlissSong],
            &'d [BlissSong],
            &'e dyn DistanceMetricBuilder,
        ) -> Box<dyn Iterator<Item = BlissSong> + 'e>,
        G: for<'b> FnMut(&'b BlissSong, &'b BlissSong) -> bool,
    {
        let mut mpd_conn = self.mpd_conn.lock().await;
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
            .skip(1)
            .collect::<Vec<_>>()
            .into_iter();

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

        self.fill_song_queue(
            &mut mpd_conn,
            &bliss_song,
            &mut playlist,
            &mut history,
            queue_length,
        )?;

        info!(
            "Time to first recommendations: {}ms",
            timestamp.elapsed().as_millis()
        );

        let mut last_queue = mpd_conn.queue()?;

        loop {
            if update_on_next_loop.load(Ordering::SeqCst) {
                drop(mpd_conn); // release lock to allow update() to acquire it
                self.update().await?;
                mpd_conn = self.mpd_conn.lock().await;
                println!("Library updated!");
                update_on_next_loop.store(false, Ordering::SeqCst);
            }
            let next_event = match mpd_conn
                .wait(&[mpd::Subsystem::Queue])
                .context("while waiting on events from MPD")
            {
                Ok(events) => events,
                Err(e) => {
                    println!("MPD connection lost, waiting to reconnect... (error: {e})");
                    Self::reconnect_to_mpd(&mut mpd_conn);
                    self.fill_song_queue(
                        &mut mpd_conn,
                        &bliss_song,
                        &mut playlist,
                        &mut history,
                        queue_length,
                    )?; // catch up on changes while disconnected
                    continue;
                }
            };

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
                        &bliss_song,
                    )?;
                }
            }
        }
    }

    fn fill_song_queue(
        &self,
        mpd_conn: &mut MutexGuard<Client<MPDStream>>,
        bliss_song: &BlissSong,
        playlist: &mut std::vec::IntoIter<BlissSong>,
        history: &mut Vec<String>,
        queue_length: u32,
    ) -> Result<()> {
        let status = mpd_conn.status()?;
        let queue_pos = status
            .song
            .ok_or(anyhow!("while getting current song position"))?
            .pos;
        let mut queue_diff = queue_length.saturating_sub(status.queue_len - queue_pos + 1);
        if (status.queue_len <= queue_length) || (queue_length > queue_diff) {
            while queue_diff > 0 {
                self.add_next_song_from_playlist(playlist, mpd_conn, history, bliss_song)?;
                queue_diff -= 1;
            }
        }
        Ok(())
    }

    /// Load genre weights from disk and associate them with tracks in the bliss library.
    ///
    /// May fail if the weights are not found,
    /// if they're in the wrong format, or if the bliss library is corrupted.
    pub fn get_track_genre_weights(
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
                    .entry(song.bliss_song.path)
                    .or_insert(collapse_genres(&genre_weights, genres));
            }
        }
        Ok(genre_weights_by_track_path)
    }

    /// Retrieve album art for a song from MPD.
    ///
    /// May fail if MPD connection is dropped or song doesn't exist in MPD database.
    pub async fn get_album_art(&self, song: &MPDSong) -> Result<Vec<u8>> {
        let mut mpd_conn = self.mpd_conn.lock().await;
        match mpd_conn
            .albumart(&song)
            .context("while getting album art from MPD")
        {
            Ok(album_art) => Ok(album_art),
            Err(_) => Ok(mpd_conn.readpicture(&song)?),
        }
    }
}
