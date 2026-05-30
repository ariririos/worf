use crate::mpd_library::{ExtraInfo, MPDLibrary, collapse_genres_pad_to};
use crate::{NUM_BLISS_FEATURES, NUM_GENRE_FEATURES};

use anyhow::{Context, anyhow};
use bliss_audio::FeaturesVersion;
use bliss_audio::library::LibrarySong as BlissSongNoInfo;
use bliss_audio::playlist::{closest_to_songs, euclidean_distance};
use itertools::Itertools;
use log::info;
use rocket::response::status::{BadRequest, NotFound};
use rocket::{State, get, http::Status, serde::json::Json};
use serde::Serialize;
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::time::Instant;

type BlissSong = BlissSongNoInfo<ExtraInfo>;

pub const CHUNK_SIZE: usize = 50;

#[derive(Serialize, Clone)]
pub struct SongAnalyses {
    pub bliss: [f32; NUM_BLISS_FEATURES],
    pub genre: [f32; NUM_GENRE_FEATURES],
}

#[derive(Serialize)]
pub struct AllSongsPage {
    page: usize,
    songs: HashMap<PathBuf, SongAnalyses>,
}

pub struct ClientLibrary {
    pub songs: ChunkedReadOnlyHashMap<PathBuf, SongAnalyses>,
    pub mpd_library: MPDLibrary,
}

#[derive(Serialize)]
pub struct ChunkedReadOnlyHashMap<K, V> {
    chunks: Vec<HashMap<K, V>>,
    total_len: usize,
}

impl<K: Hash + Eq + Clone + Ord, V: Clone> ChunkedReadOnlyHashMap<K, V> {
    pub fn new(base: HashMap<K, V>, chunk_size: usize) -> Self {
        let total_len = base.len();
        let num_chunks = total_len / chunk_size + 1;
        let chunked_entries = base
            .into_iter()
            .sorted_by_cached_key(|x| x.0.clone())
            .chunks(chunk_size);
        let chunks: Vec<_> = (&chunked_entries)
            .into_iter()
            .map(HashMap::from_iter)
            .collect();
        assert!(num_chunks == chunks.len());
        Self { chunks, total_len }
    }

    pub fn get_chunk_index(&self, key: &K) -> Option<usize> {
        for (i, chunk) in self.chunks.iter().enumerate() {
            if chunk.contains_key(key) {
                return Some(i);
            }
        }
        None
    }

    pub fn get(&self, key: K) -> Option<V> {
        let idx = self.get_chunk_index(&key)?;
        self.chunks[idx].get(&key).cloned()
    }

    pub fn get_chunk_at_index(&self, idx: usize) -> Option<&HashMap<K, V>> {
        self.chunks.get(idx)
    }
}

#[get("/all")]
pub fn info(state: &State<ClientLibrary>) -> Json<Info> {
    Json(Info {
        max_page: state.songs.chunks.len() - 1,
    })
}

#[get("/all/<page>")]
pub fn all(
    page: usize,
    state: &State<ClientLibrary>,
) -> std::result::Result<Json<AllSongsPage>, Status> {
    if page >= state.songs.chunks.len() {
        return Err(Status::BadRequest);
    }
    Ok(Json(AllSongsPage {
        page,
        songs: state
            .songs
            .get_chunk_at_index(page)
            .ok_or(Status::InternalServerError)?
            .iter()
            .map(|(key, val)| (key.clone(), val.clone()))
            .collect(),
    }))
}

#[derive(Serialize)]
pub struct Info {
    max_page: usize,
}

#[get("/analysis/<path>")]
pub fn analysis(
    path: &str,
    state: &State<ClientLibrary>,
) -> std::result::Result<Json<SongAnalyses>, NotFound<String>> {
    Ok(Json(
        state
            .songs
            .get(path.into())
            .context("Song does not exist in bliss database")
            .map_err(|e| NotFound(e.to_string()))?,
    ))
}

#[derive(Serialize, Clone)]
pub struct ClientPlaylistSong {
    path: PathBuf,
    artist: Option<String>,
    title: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<i32>,
    disc_number: Option<i32>,
    genre: Option<String>,
    analysis: SongAnalyses,
    duration: u64,
    features_version: FeaturesVersion,
}

#[derive(Serialize)]
pub struct ClientPlaylist {
    head: ClientPlaylistSong,
    tail: Vec<ClientPlaylistSong>,
}

#[get("/playlist/<path>?<length>")]
pub fn playlist(
    path: &str,
    length: usize,
    state: &State<ClientLibrary>,
) -> std::result::Result<Json<ClientPlaylist>, BadRequest<String>> {
    if length == 0 {
        return Err(BadRequest(
            "Playlist length must be greater than zero".into(),
        ));
    }
    let bliss_sort = |x: &[BlissSong], y: &[BlissSong], z| -> Box<dyn Iterator<Item = BlissSong>> {
        Box::new(closest_to_songs(x, y, z))
    };
    let full_song_path = PathBuf::new()
        .join(state.mpd_library.bliss.config.mpd_base_path.clone())
        .join(path);
    let now = Instant::now();
    let playlist: Vec<ClientPlaylistSong> = state
        .mpd_library
        .bliss
        .playlist_from_custom(
            &[full_song_path
                .to_str()
                .ok_or(BadRequest("Song path wasn't valid Unicode".into()))?],
            &euclidean_distance,
            bliss_sort,
            true,
        )
        .context("while building bliss playlist")
        .map_err(|e| BadRequest(e.to_string()))?
        .map(|song| {
            let bliss_song = song.bliss_song;
            ClientPlaylistSong {
                path: bliss_song
                    .path
                    .strip_prefix(&state.mpd_library.bliss.config.mpd_base_path)
                    .expect("failed to strip MPD base path")
                    .to_path_buf(),
                artist: bliss_song.artist,
                title: bliss_song.title,
                album: bliss_song.album,
                album_artist: bliss_song.album_artist,
                track_number: bliss_song.track_number,
                disc_number: bliss_song.disc_number,
                genre: bliss_song.genre.clone(),
                analysis: SongAnalyses {
                    bliss: *bliss_song
                        .analysis
                        .as_vec()
                        .as_array()
                        .expect("while converting bliss analysis to array"),
                    genre: {
                        let current_genre = Some(bliss_song.genre.unwrap_or_default());
                        collapse_genres_pad_to(
                            &state
                                .mpd_library
                                .genre_weights
                                .clone()
                                .ok_or(anyhow!("while getting genre weights"))
                                .expect("while getting genre weights"),
                            current_genre
                                .as_ref()
                                .ok_or(anyhow!("while getting current genre weight"))
                                .expect("while getting current genre weight")
                                .clone(),
                        )
                    },
                },
                duration: bliss_song.duration.as_secs(),
                features_version: bliss_song.features_version,
            }
        })
        .take(length + 1)
        .collect();

    info!("Playlist generated in {}ms", now.elapsed().as_millis());

    Ok(Json(ClientPlaylist {
        head: playlist[0].clone(),
        tail: playlist[1..].to_vec(),
    }))
}
