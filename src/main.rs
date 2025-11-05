// Worf 0.0.1
// * Copyright (c) 2025 Ari Rios <me@aririos.com>
// * License-SPDX: GPL-3.0-only
// * Based on Polochon-street/blissify-rs
//!
//! Worf is a daemon that automatically queues songs in MPD based off a "pinned song", which is just whatever song was playing when the daemon was started.
//! It uses bliss-audio to queue songs that are most similar to the pinned song. The pinned song is changed simply by playing a new song.

// TODO:
// - Move beyond just using bliss and integrate last.fm similar artists and/or genre tags.
// - In a separate thread, keep the bliss database updated as new songs are added to MPD.
// - Ultimately, the idea of keeping state about the "pinned song" requires a whole new MPD client -- none of the current ones have an idea of "song radio", "artist radio", etc.

mod mpd_library;

use anyhow::{Context, Result, anyhow, bail};
use bliss_audio::library::LibrarySong as BlissSong;
use bliss_audio::playlist::{closest_to_songs, euclidean_distance};
use clap::{Parser, Subcommand};
// use extended_isolation_forest::ForestOptions;
use itertools::Itertools;
use mpd::Song as MPDSong;
use mpd_library::{MPDLibrary, collapse_genres, slice_as_array, closest_to_genre_songs};
use ndarray::arr1;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 10)]
    queue_length: u8,
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
    Genres,
    Daemon,
    Update,
    Init,
}

struct PinnedSong(MPDSong);

fn main() -> Result<()> {
    let args = Args::parse();

    match &args.command {
        Some(Commands::Genres) => {
            println!("Queueing {} songs and daemonizing...", args.queue_length);
            let config_path = args.config_path;
            let mut mpd_library = match MPDLibrary::retrieve(config_path.clone()) {
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
            let track_weights = mpd_library.get_track_genre_weights()?;

            let genre_sort = |x: &[BlissSong<()>],
                              y: &[BlissSong<()>],
                              z|
             -> Box<dyn Iterator<Item = BlissSong<()>>> {
                Box::new(closest_to_genre_songs(x, y, z, &track_weights))
            };

            let bliss_sort = |x: &[BlissSong<()>],
                              y: &[BlissSong<()>],
                              z|
             -> Box<dyn Iterator<Item = BlissSong<()>>> {
                Box::new(closest_to_songs(x, y, z))
            };

            let Ok(Some(current_song)) = mpd_library.mpd_conn.lock().expect("Poisoned lock").currentsong() else {
                bail!("Start playing a song and try again.");
            };

            let mut pinned_song = PinnedSong(current_song);

            loop {
                let current_genre = mpd_library
                    .mpd_to_bliss_song(&pinned_song.0)
                    .context("while getting current song genre")?
                    .ok_or(anyhow!("while getting current song genre"))?
                    .bliss_song
                    .genre
                    .unwrap_or_default();
                let current_genre_weight = collapse_genres(
                    &mpd_library.genre_weights.clone().ok_or(anyhow!("while getting genre weights"))?,
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
                            &arr1(slice_as_array::<5, f32>(&current_genre_weight).expect("while converting genre weight slice to array")),
                            &arr1(slice_as_array::<5, f32>(a.1).expect("while converting genre weight slice to array"))
                        )
                        .total_cmp(&euclidean_distance(
                            &arr1(slice_as_array::<5, f32>(&current_genre_weight).expect("while converting genre weight slice to array")),
                            &arr1(slice_as_array::<5, f32>(b.1).expect("while converting genre weight slice to array"))
                        )))
                        .map(|(name, _)| name)
                        .take(10)
                        .join(",")
                );

                // TODO: once #![feature(closure_lifetime_binder)] is stable, combine these if/else branches
                if current_genre.is_empty() {
                    println!("No genre available, defaulting to bliss analysis queueing...");
                    match mpd_library.queue_from_song(
                        &pinned_song.0,
                        10,
                        &euclidean_distance,
                        bliss_sort,
                        true,
                        false,
                    ) {
                        Ok(next_pinned) => {
                            if next_pinned == pinned_song.0 {
                                println!("You made it to the end of your music library!");
                                return Ok(());
                            }
                            pinned_song = PinnedSong(next_pinned);
                            println!(
                                "Restarting with new pinned song: {}",
                                pinned_song.0.title.as_ref().ok_or(anyhow!("while getting pinned song title"))?
                            );
                            continue;
                        }
                        Err(e) => bail!(e),
                    }
                } else {
                    match mpd_library.queue_from_song(
                        &pinned_song.0,
                        10,
                        &euclidean_distance,
                        genre_sort,
                        true,
                        false,
                    ) {
                        Ok(next_pinned) => {
                            if next_pinned == pinned_song.0 {
                                println!("You made it to the end of your music library!");
                                return Ok(());
                            }
                            pinned_song = PinnedSong(next_pinned);
                            println!(
                                "Restarting with new pinned song: {}",
                                pinned_song.0.title.as_ref().ok_or(anyhow!("while getting pinned song title"))?
                            );
                            continue;
                        }
                        Err(e) => bail!(e),
                    }
                }
            }
        }
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

            let Ok(Some(current_song)) = mpd_library.mpd_conn.lock().expect("Poisoned lock").currentsong() else {
                bail!("Start playing a song and try again.");
            };

            let mut pinned_song = PinnedSong(current_song);

            println!(
                "Queueing from pinned song: {}",
                pinned_song.0.title.as_ref().ok_or(anyhow!("Failed to get pinned song title"))?
            );

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
                    &sort,
                    true,
                    false,
                ) {
                    Ok(next_pinned) => {
                        if next_pinned == pinned_song.0 {
                            println!("You made it to the end of your music library!");
                            return Ok(());
                        }
                        pinned_song = PinnedSong(next_pinned);
                        println!(
                            "Restarting with new pinned song: {}",
                            pinned_song.0.title.as_ref().ok_or(anyhow!("Failed to get pinned song title"))?
                        );
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
