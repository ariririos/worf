import * as d3 from "d3";

const margin = { top: 10, right: 30, bottom: 30, left: 60 },
    width = 1280 - margin.left - margin.right,
    height = 720 - margin.top - margin.bottom;

// will get reused
const tooltip = d3.select("body")
    .append("div")
    .style("position", "absolute")
    .style("background", "rgba(0, 0, 0, 0.8)")
    .style("color", "white")
    .style("padding", "8px 12px")
    .style("border-radius", "4px")
    .style("font-size", "12px")
    .style("pointer-events", "none")
    .style("opacity", 0)
    .style("z-index", 1000);

const svg = d3.select("#container")
    .append("svg")
    .attr("width", width + margin.left + margin.right)
    .attr("height", height + margin.top + margin.bottom)
    .append("g")
    .attr("transform", "translate(" + margin.left + "," + margin.top + ")");

function collapseArray(arr) {
    let total = arr.reduce((sum, val) => sum + val, 0);
    return total / arr.length;
}

let songPath = "Shygirl/Nymph_o/07 - Shygirl - Playboy _ Positions.ogg";

d3.json("http://localhost:8080/api/playlist/" + encodeURIComponent(songPath) + "?length=150")
    .then(function (data) {

        const blissValues = data.tail.flatMap(d => collapseArray(d.analysis.bliss));
        const genreValues = data.tail.flatMap(d => collapseArray(d.analysis.genre));

        // bliss on the x
        const x = d3.scaleLinear()
            .domain(d3.extent(blissValues)).nice()
            .range([0, width]);

        // genre on the y
        const y = d3.scaleLinear()
            .domain(d3.extent(genreValues)).nice()
            .range([height, 0]);

        svg.append("g")
            .attr("transform", "translate(0," + height + ")")
            .call(d3.axisBottom(x).ticks(5));

        svg.append("g")
            .call(d3.axisLeft(y).ticks(5));

        const selectedSongTitle = data.head.title || data.head.path.split("/").pop() || songPath;
        svg.append("text")
            .attr("x", width / 2)
            .attr("y", -margin.top / 2)
            .attr("text-anchor", "middle")
            .style("font-size", "18px")
            .style("font-weight", "bold")
            .text(`Selected song: ${selectedSongTitle}`);

        // axis labels
        svg.append("text")
            .attr("transform", "translate(" + (width / 2) + ")")
            .attr("x", -10)
            .attr("y", height + 30)
            .style("text-anchor", "middle")
            .text("Bliss Similarity");

        svg.append("text")
            .attr("transform", "rotate(-90)")
            .attr("y", 0 - margin.left)
            .attr("x", 0 - (height / 2))
            .attr("dy", "1em")
            .style("text-anchor", "middle")
            .text("Genre Similarity");

        const points = data.tail.map(d => ({ bliss: d.analysis.bliss, genre: d.analysis.genre, i: data.tail.indexOf(d) }));
        const audio = new Audio();
        const imageSize = 40;

        const createHoverHandlers = (dataPoint) => ({
            mouseover: function (event) {
                const item = data.tail[dataPoint.i];
                d3.select(this).style("opacity", 1);
                tooltip
                    .style("opacity", 1)
                    .html(`
                        <strong><div style="text-decoration: underline;">${item.title}</div></strong>
                        <br/>
                        <strong>Bliss distance:</strong> ${collapseArray(dataPoint.bliss).toFixed(3)}
                        <br/>
                        <strong>Genre distance:</strong> ${collapseArray(dataPoint.genre).toFixed(3)}
                        <br/>
                        <strong>Genres:</strong> ${item.genre}
                        <br/>
                        <strong>Popularity:</strong> ${item.popularity !== undefined ? item.popularity : 'N/A'}
                    `)
                    .style("left", (event.pageX + 10) + "px")
                    .style("top", (event.pageY - 28) + "px");
                audio.src = item.href;
                audio.currentTime = 30;
                audio.play().catch(err => console.log("Audio playback failed:", err));
            },
            mouseout: function () {
                d3.select(this).style("opacity", 0.8);
                tooltip.style("opacity", 0);
                audio.pause();
            }
        });

        const imageSelection = svg.append('g')
            .selectAll("image")
            .data(points)
            .enter()
            .append("image")
            .attr("x", d => x(collapseArray(d.bliss)) - imageSize / 2)
            .attr("y", d => y(collapseArray(d.genre)) - imageSize / 2)
            .attr("width", imageSize)
            .attr("height", imageSize)
            .attr("href", d => {
                const songPath = data.tail[d.i].path;
                return "http://localhost:8080/api/albumart/" + encodeURIComponent(songPath);
            })
            .style("cursor", "pointer")
            .style("opacity", 0.8)
            .on("error", function () {
                const d = d3.select(this).datum();
                const cx = x(collapseArray(d.bliss));
                const cy = y(collapseArray(d.genre));

                // image failed to load => use a circle instead
                d3.select(this).remove();

                const handlers = createHoverHandlers(d);
                svg.append("circle")
                    .attr("cx", cx)
                    .attr("cy", cy)
                    .attr("r", imageSize / 2)
                    .style("fill", "#69b3a2")
                    .style("opacity", 0.8)
                    .style("cursor", "pointer")
                    .on("mouseover", handlers.mouseover)
                    .on("mouseout", handlers.mouseout);
            })
            .on("mouseover", function (event, d) {
                const handlers = createHoverHandlers(d);
                handlers.mouseover.call(this, event);
            })
            .on("mouseout", function (event, d) {
                const handlers = createHoverHandlers(d);
                handlers.mouseout.call(this, event);
            });

        console.log("Graph rendered with", points.length, "points");
    })
    .catch(function (error) {
        console.error("Error loading data:", error);
    });
