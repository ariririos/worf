# Worf
Worf is a daemon that automatically queues songs in MPD based on a "pinned song", which is just whatever song was playing when the daemon was started.
It uses bliss-audio to queue songs that are most similar to the pinned song. The pinned song is changed simply by playing a new song.

# Usage
First, initialize the bliss library if not already done by running `cargo run -- --base-path $MPD_BASE_PATH init` in the project root, where `$MPD_BASE_PATH` is where music is stored (can be a network location with username/password).
Once that's done, run `cargo run -- daemon` in the project root to start queueing similar songs.
Use `cargo run -- update` to update the bliss library with new songs from MPD.
