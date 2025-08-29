# worf
Connects an existing blissify database created by [blissify](https://github.com/polochon-street/blissify-rs) to MPD and creates custom playlists.

# Build
Just run `make`!

# Usage
In environment variables or a `.env` file in the project root, include:
```
MPD_HOST=`MPD server host` (optional, defaults to localhost)
MPD_PORT=`MPD server port` (optional, defaults to MPD default, usually 6600)
MPD_PASSWORD=`MPD server password` (optional)
BLISSIFY_PASSWORD=`MPD password for the server running blissify` (optional)
BLISS_DB=`location of the bliss-audio songs.db, usually ~/.config/bliss-rs/songs.db` **required**
MUSIC_DIR=`MPD music directory` **required**
```

On the command line:
`./worf`

With option `--song-id`, playlist is based on Euclidean distance from that song. Song IDs are the `id` column in the `song` table in the BLISS_DB.
With `--song-glob`, will look up song from database based on a glob pattern and then create a playlist based on Euclidean distance.
Otherwise, playlist is randomly generated.

Other options:
`run-blissify-update`: whether to run `blissify update` to bring the blissify database up to date with the MPD server, defaults to true.
`length`: length of the generated playlist, defaults to 50.
`playlist-name`: name of the generated playlist, defaults to `bliss-playlist`.
