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

mod mpd_library;
mod server;

use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::library::LibrarySong as BlissSong;
use bliss_audio::playlist::{closest_to_songs, euclidean_distance};
use clap::{Parser, Subcommand};
use itertools::Itertools;
use mpd::Song as MPDSong;
use mpd_library::{MPDLibrary, closest_to_genre_songs, collapse_genres, slice_as_array};
use ndarray::arr1;
use rocket::Config;
use rocket::fs::{FileServer, Options, relative};
use rocket::routes;
use server::{CHUNK_SIZE, ChunkedReadOnlyHashMap, ClientLibrary, all, analysis, info, playlist};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

const NUM_GENRE_FEATURES: usize = 5;

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
    /// Pass to update bliss library on `genres`, `daemon`, and `server` commands
    update_library: bool,
    #[arg(short, long)]
    /// Path of genre map JSON file
    genres_path: Option<PathBuf>,
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

#[rocket::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config_path;

    let bliss_sort =
        |x: &[BlissSong<()>], y: &[BlissSong<()>], z| -> Box<dyn Iterator<Item = BlissSong<()>>> {
            Box::new(closest_to_songs(x, y, z))
        };

    match &args.command {
        Some(Commands::Genres) => {
            println!("Queueing songs in background...");
            let mut mpd_library = MPDLibrary::retrieve(config_path.clone())?;
            let track_weights =
                mpd_library.get_track_genre_weights::<NUM_GENRE_FEATURES>(args.genres_path)?;

            let genre_sort = |x: &[BlissSong<()>],
                              y: &[BlissSong<()>],
                              z|
             -> Box<dyn Iterator<Item = BlissSong<()>>> {
                Box::new(closest_to_genre_songs(x, y, z, &track_weights))
            };

            if args.update_library {
                mpd_library
                    .bliss
                    .update_library(mpd_library.get_all_mpd_songs()?, true, true)
                    .context("while updating bliss library")?;
            }

            let current_song = mpd_library.get_current_song()?;

            let mut pinned_song = PinnedSong(current_song);

            println!(
                "Starting with pin {}",
                pinned_song.0.title.clone().unwrap_or("Unknown".to_string())
            );

            loop {
                let current_genre = mpd_library
                    .mpd_to_bliss_song(&pinned_song.0)
                    .context("while getting current song genre")?
                    .ok_or(anyhow!("while getting current song genre"))?
                    .bliss_song
                    .genre
                    .unwrap_or_default();
                let current_genre_weight = collapse_genres::<NUM_GENRE_FEATURES>(
                    &mpd_library
                        .genre_weights
                        .clone()
                        .ok_or(anyhow!("while getting genre weights"))?,
                    current_genre.clone(),
                );
                println!("Current genre: {}", current_genre);
                println!(
                    "Closest 10 genres: {}",
                    mpd_library
                        .genre_weights
                        .clone()
                        .ok_or(anyhow!("while getting genre weights"))?
                        .iter()
                        .sorted_by(|a, b| euclidean_distance(
                            &arr1(
                                slice_as_array::<5, f32>(&current_genre_weight)
                                    .expect("while converting genre weight slice to array")
                            ),
                            &arr1(
                                slice_as_array::<5, f32>(a.1)
                                    .expect("while converting genre weight slice to array")
                            )
                        )
                        .total_cmp(&euclidean_distance(
                            &arr1(
                                slice_as_array::<5, f32>(&current_genre_weight)
                                    .expect("while converting genre weight slice to array")
                            ),
                            &arr1(
                                slice_as_array::<5, f32>(b.1)
                                    .expect("while converting genre weight slice to array")
                            )
                        )))
                        .map(|(name, _)| name)
                        .take(10)
                        .join(",")
                );

                // TODO: once #![feature(closure_lifetime_binder)] is stable, combine these if/else branches
                if current_genre.is_empty() {
                    println!("No genre available, defaulting to bliss analysis queueing...");
                    pinned_song = PinnedSong(mpd_library.queue_from_song(
                        &pinned_song.0,
                        10,
                        &euclidean_distance,
                        bliss_sort,
                        true,
                        true,
                        std::time::Instant::now(),
                    )?)
                } else {
                    pinned_song = PinnedSong(mpd_library.queue_from_song(
                        &pinned_song.0,
                        10,
                        &euclidean_distance,
                        genre_sort,
                        true,
                        true,
                        std::time::Instant::now(),
                    )?)
                }
            }
        }
        Some(Commands::Daemon) => {
            println!("Queueing songs in background...");
            let mut mpd_library = MPDLibrary::retrieve(config_path.clone())?;

            if args.update_library {
                mpd_library
                    .bliss
                    .update_library(mpd_library.get_all_mpd_songs()?, true, true)
                    .context("while updating bliss library")?;
            }

            let current_song = mpd_library.get_current_song()?;

            let mut pinned_song = PinnedSong(current_song);

            println!(
                "Queueing from pin: {}",
                pinned_song
                    .0
                    .title
                    .as_ref()
                    .ok_or(anyhow!("while getting pin title"))?
            );

            // let forest_distance: &dyn DistanceMetricBuilder = &ForestOptions {
            //     n_trees: 1000,
            //     sample_size: 200,
            //     max_tree_depth: None,
            //     extension_level: 10,
            // }; // this seems to only work right with multiple songs; with only one song as the pin, it always generates the same playlist

            loop {
                pinned_song = PinnedSong(mpd_library.queue_from_song(
                    &pinned_song.0,
                    10,
                    &euclidean_distance,
                    bliss_sort,
                    true,
                    true,
                    std::time::Instant::now(),
                )?)
            }
        }
        Some(Commands::Server { bind_to }) => {
            let bind = bind_to.clone().unwrap_or("127.0.0.1:8080".to_string());

            let mut mpd_library = MPDLibrary::retrieve(config_path.clone())?;

            if args.update_library {
                mpd_library
                    .bliss
                    .update_library(mpd_library.get_all_mpd_songs()?, true, true)
                    .context("while updating bliss library")?;
            }

            let songs: HashMap<PathBuf, Vec<f32>> = mpd_library
                .bliss
                .songs_from_library::<()>()?
                .iter()
                .map(|song| {
                    (
                        song.bliss_song
                            .path
                            .strip_prefix(mpd_library.bliss.config.mpd_base_path.to_path_buf())
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
            mpd_library
                .bliss
                .update_library(mpd_library.get_all_mpd_songs()?, true, true)
        }
        None => {
            bail!("No command provided!");
        }
    }
}
