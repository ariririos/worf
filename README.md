# Worf
Worf is a daemon that automatically queues songs in MPD based on a "pinned song", which is just whatever song was playing when the daemon was started.
It uses bliss-audio to queue songs that are most similar to the pinned song. The pinned song is changed simply by playing a new song.

# Usage
First, initialize the bliss library if not already done by running `worf --base-path $MPD_BASE_PATH init` in the project root, where `$MPD_BASE_PATH` is where music is stored (can be a network location with username/password).
Once that's done, run `worf daemon` in the project root to start queueing similar songs.
Use `worf update` to update the bliss library with new songs from MPD.

## By genres (experimental!)
Credit to Glenn McDonald and Spotify for the [Every Noise at Once](https://everynoise.com) project, which provides similarity metrics for Spotify's genre tags. If your music is tagged accordingly, such as with [Zotify](https://github.com/Googolplexed0/zotify) (or my [zotify-genre-tagger](https://github.com/ariririos/zotify-genre-tagger) if you forgot to enable genre tagging), worf can queue music by genre similarity. While this prevents the sort of "drifting" that purely audio-based similarity metrics might cause, in my experience, it often leads to the opposite problem of staying too close in a genre bubble.

You can try it with `worf genres`. You will need a genres.json file in the same directory, with genre names as the keys and a 5-element array of integers as the values. I won't post it directly here, but you can acquire your own copy with this Javascript on everynoise.com:
```Javascript
let weights = {};
for (let node of Array.from(document.querySelectorAll(".canvas .genre"))) {
    weights[node.innerText] = [parseInt(node.style.top), parseInt(node.style.left), parseInt(node.style.color.split(",")[0].split("rgb(")[1]), parseInt(node.style.color.split(",")[1]), parseInt(node.style.color.split(",")[2].split(")")[0])]
}
JSON.stringify(weights);
```
