// Worf
// * Copyright (c) 2025 Ari Rios <me@aririos.com>
// * License-SPDX: GPL-3.0-only
// * Based on Polochon-street/blissify-rs
//!
//! Worf is a daemon that automatically queues songs in MPD based off a "pin", which is just whatever song was playing when the daemon was started.
//! It uses bliss-audio to queue songs that are most similar to the pin. The pin is changed simply by playing a new song.

// TODO:
// - Move beyond just using bliss and integrate last.fm similar artists and/or genre tags.
// - In a separate thread, keep the bliss database updated as new songs are added to MPD.
// - Ultimately, the idea of keeping state about the "pin" requires a whole new MPD client -- none of the current ones have an idea of "song radio", "artist radio", etc.
// - some method of keeping track of skips, replays, etc. and some method of integrating them into the playlist
// - I would love to integrate information from the whosampled database
// - genre sort is the same exact playlist for any two songs with the same genre tags; integrate secondary bliss sorting somehow (maybe do it in chunks of 100 or something)
// - would be nice to have a way to exclude a song from recommendations completely
// - restart with current song as pin on SIGHUP
// - switch between genres/bliss/future modes with SIGUSR1
// - is there some way to do caching? but you immediately run into a cache invalidation at the next update; maybe the update thread will be responsible for updating the cache?
// - popularity filter on recommendations; gonna need to figure out the tagging (done!)

mod mpd_library;
mod server;

use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::playlist::{closest_to_songs, euclidean_distance};
use clap::{Parser, Subcommand};
// use futures::stream::StreamExt;
use itertools::Itertools;
use mpd::Song as MPDSong;
use mpd_library::{BlissSong, MPDLibrary, closest_to_genre_songs, collapse_genres, slice_as_array};
use ndarray::arr1;
use rocket::Config;
use rocket::fs::{FileServer, Options, relative};
use rocket::routes;
use server::{CHUNK_SIZE, ChunkedReadOnlyHashMap, ClientLibrary, all, analysis, info, playlist};
// use signal_hook::consts::signal::*;
// use signal_hook_tokio::Signals;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

pub const NUM_GENRE_FEATURES: usize = 5;
const POPULARITY_DIFFERENCE_FLOOR: i32 = 10;

#[derive(Parser, Debug)]
#[command(name = "Worf", version, about, long_about = None)]
struct Args {
    /// MPD base path
    #[arg(short, long)]
    base_path: Option<PathBuf>,
    /// Bliss config path
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Bliss database path
    #[arg(short, long)]
    database_path: Option<PathBuf>,
    /// MPD password
    #[arg(short, long)]
    password: Option<String>,
    #[arg(short, long)]
    /// Pass to update bliss library once at start on `genres`, `daemon`, and `server` commands
    update_library: bool,
    #[arg(short, long)]
    /// Path of genre map JSON file
    genres_path: Option<PathBuf>,
    // #[arg(short = 'k', long)]
    // /// Keep the bliss database updated in a separate thread when the MPD database updates on `genres`, `daemon`, and `server` modes
    // keep_updated: bool,
    #[arg(short = 'f', long)]
    /// Only recommend songs at least as popular as the pin, within 10% (only for `daemon` and `genres`, requires songs tagged with popularity -- see README)
    popularity_filter: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Queue songs based on genre similarity
    Genres,
    /// Queue songs based on audio similarity from bliss
    Daemon,
    /// Serve analysis over the network
    Server {
        /// Where to bind the server. Possible formats are `address`, `address:port`, `:port`
        bind_to: Option<String>,
    },
    /// Update bliss library
    Update,
    /// Initialize (or reinitialize) bliss library
    Init,
}

struct PinnedSong(MPDSong);

// fn spawn_update_thread(mpd_conn: Client<MPDStream>) {}

// async fn handle_signals(mut signals: Signals) {
//     while let Some(signal) = signals.next().await {
//         match signal {
//             SIGHUP => {
//                 println!("caught SIGHUP");
//             }
//             SIGUSR1 => {
//                 println!("caught SIGUSR1");
//             }
//             _ => unreachable!(),
//         }
//     }
// }

#[rocket::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();
    let config_path = args.config_path;

    let bliss_sort = |x: &[BlissSong], y: &[BlissSong], z| -> Box<dyn Iterator<Item = BlissSong>> {
        Box::new(closest_to_songs(x, y, z))
    };

    // let signals = Signals::new(&[SIGHUP, SIGUSR1])?;
    // let handle = signals.handle();
    // let signals_task = rocket::tokio::spawn(handle_signals(signals));

    let popularity_filter = |current_song: &BlissSong, original_song: &BlissSong| -> bool {
        current_song.extra_info.popularity
            > original_song.extra_info.popularity - POPULARITY_DIFFERENCE_FLOOR
    };

    match &args.command {
        Some(Commands::Genres) | Some(Commands::Daemon) => {
            println!("Queueing songs in background...");
            let mut mpd_library = MPDLibrary::retrieve(config_path.clone())?;

            let track_weights = if let Some(Commands::Genres) = args.command {
                mpd_library.get_track_genre_weights::<NUM_GENRE_FEATURES>(args.genres_path)?
            } else {
                HashMap::new()
            };

            let genre_sort =
                |x: &[BlissSong], y: &[BlissSong], z| -> Box<dyn Iterator<Item = BlissSong>> {
                    Box::new(closest_to_genre_songs(x, y, z, &track_weights))
                };

            if args.update_library {
                mpd_library.update()?;
            }

            let current_song = mpd_library.get_current_song()?;

            let mut pinned_song = PinnedSong(current_song);

            println!(
                "Starting with pin {}",
                pinned_song.0.title.clone().unwrap_or("Unknown".to_string())
            );

            // let forest_distance: &dyn DistanceMetricBuilder = &ForestOptions {
            //     n_trees: 1000,
            //     sample_size: 200,
            //     max_tree_depth: None,
            //     extension_level: 10,
            // }; // this seems to only work right with multiple songs; with only one song as the pin, it always generates the same playlist

            loop {
                let current_genre: Option<String>;
                if let Some(Commands::Genres) = args.command {
                    current_genre = Some(
                        mpd_library
                            .mpd_to_bliss_song(&pinned_song.0)
                            .context("while getting current song genre")?
                            .ok_or(anyhow!("while getting current song genre"))?
                            .bliss_song
                            .genre
                            .unwrap_or_default(),
                    );
                    let current_genre_weight = collapse_genres::<NUM_GENRE_FEATURES>(
                        &mpd_library
                            .genre_weights
                            .clone()
                            .ok_or(anyhow!("while getting genre weights"))?,
                        // this unwrap is fine because we just set current_genre to Some
                        current_genre.as_ref().unwrap().clone(),
                    );
                    if let Some(genre) = current_genre.as_ref()
                        && !genre.is_empty()
                    {
                        println!("Current genre: {}", current_genre.as_ref().unwrap());
                        println!(
                            "Closest 10 genres: {}",
                            mpd_library
                                .genre_weights
                                .clone()
                                .ok_or(anyhow!("while getting genre weights"))?
                                .iter()
                                .sorted_by(|a, b| euclidean_distance(
                                    &arr1(
                                        slice_as_array::<NUM_GENRE_FEATURES, f32>(
                                            &current_genre_weight
                                        )
                                        .expect("while converting genre weight slice to array")
                                    ),
                                    &arr1(
                                        slice_as_array::<NUM_GENRE_FEATURES, f32>(a.1)
                                            .expect("while converting genre weight slice to array")
                                    )
                                )
                                .total_cmp(&euclidean_distance(
                                    &arr1(
                                        slice_as_array::<NUM_GENRE_FEATURES, f32>(
                                            &current_genre_weight
                                        )
                                        .expect("while converting genre weight slice to array")
                                    ),
                                    &arr1(
                                        slice_as_array::<NUM_GENRE_FEATURES, f32>(b.1)
                                            .expect("while converting genre weight slice to array")
                                    )
                                )))
                                .map(|(name, _)| name)
                                .take(10)
                                .join(",")
                        );
                    }
                } else {
                    current_genre = None;
                }

                if let Some(ref genre) = current_genre
                    && genre.is_empty()
                {
                    println!("Pin has no genre, using bliss similarity");
                }

                let sort_fn: Box<
                    dyn Fn(&[BlissSong], &[BlissSong], _) -> Box<dyn Iterator<Item = BlissSong>>,
                > = if current_genre.as_ref().is_none_or(|genre| genre.is_empty()) {
                    Box::new(bliss_sort)
                } else {
                    Box::new(genre_sort)
                };

                pinned_song = PinnedSong(mpd_library.queue_from_song(
                    &pinned_song.0,
                    10,
                    &euclidean_distance,
                    sort_fn,
                    Some(popularity_filter),
                    true,
                    true,
                    std::time::Instant::now(),
                )?);
            }
        }
        Some(Commands::Server { bind_to }) => {
            let bind = bind_to.clone().unwrap_or("127.0.0.1:8080".to_string());

            let mut mpd_library = MPDLibrary::retrieve(config_path.clone())?;

            if args.update_library {
                mpd_library.update()?;
            }

            let songs: HashMap<PathBuf, Vec<f32>> = mpd_library
                .bliss
                .songs_from_library::<()>()?
                .iter()
                .map(|song| {
                    (
                        song.bliss_song
                            .path
                            .strip_prefix(&mpd_library.bliss.config.mpd_base_path)
                            .context("while stripping MPD base path from song path")
                            .unwrap()
                            .to_path_buf(),
                        song.bliss_song.analysis.as_vec(),
                    )
                })
                .collect();

            let library_interface = ClientLibrary {
                songs: ChunkedReadOnlyHashMap::new(songs, CHUNK_SIZE),
                mpd_library,
            };

            let (address, port) = match bind.split_once(':') {
                Some(("", port)) => (
                    "127.0.0.1",
                    port.parse::<u16>()
                        .context("while trying to parse server port")?,
                ),
                Some((address, "")) => (address, 8080),
                Some((address, port)) => (
                    address,
                    port.parse::<u16>()
                        .context("while trying to parse server port")?,
                ),
                None => {
                    // no IPv6 but could probably fix that by splitting only on the last `:`
                    if Ipv4Addr::from_str(&bind).is_ok() {
                        (bind.as_str(), 8080)
                    } else {
                        println!(
                            "Error parsing server bind address or port, using default 127.0.0.1:8080"
                        );
                        ("127.0.0.1", 8080)
                    }
                }
            };

            let figment = Config::figment()
                .merge(("address", address))
                .merge(("port", port));

            rocket::custom(figment)
                .mount("/", FileServer::new(relative!("public"), Options::Index))
                .mount("/api/", routes![all, info, analysis, playlist])
                // .register("/", catchers![not_found])
                .manage(library_interface)
                .launch()
                .await
                .context("while starting Rocket server")?;

            Ok(())
        }
        Some(Commands::Init) => {
            println!("Initializing music library and analyzing...");
            let database_path = args.database_path;
            let Some(mpd_base_path) = args.base_path else {
                bail!("--base-path must be used if running `init`");
            };
            let mut mpd_library = MPDLibrary::build(mpd_base_path, config_path, database_path)?;
            mpd_library.populate()
        }
        Some(Commands::Update) => {
            println!("Updating music library analysis...");
            let mut mpd_library = MPDLibrary::retrieve(config_path)?;
            mpd_library.update()
        }
        None => {
            bail!("No command provided!");
        }
    }
}
