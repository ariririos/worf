// import * as d3 from "d3";

// const margin = { top: 10, right: 30, bottom: 30, left: 60 },
//     width = 460 - margin.left - margin.right,
//     height = 400 - margin.top - margin.bottom;

// const svg = d3.select("#container")
//     .append("svg")
//     .attr("width", width + margin.left + margin.right)
//     .attr("height", height + margin.top + margin.bottom)
//     .append("g")
//     .attr("transform",
//         "translate(" + margin.left + "," + margin.top + ")");

// d3.json("http://localhost:8080/api/playlist/Shygirl%2FNymph_o%2F07%20-%20Shygirl%20-%20Playboy%20_%20Positions.ogg?length=20", function (data) {
//     console.log("data", data);
//     var x = d3.scaleLinear()
//         .domain([0, 4000])
//         .range([0, width]);
//     svg.append("g")
//         .attr("transform", "translate(0," + height + ")")
//         .call(d3.axisBottom(x));

//     // Add Y axis
//     var y = d3.scaleLinear()
//         .domain([0, 500000])
//         .range([height, 0]);
//     svg.append("g")
//         .call(d3.axisLeft(y));

//     // Add dots
//     svg.append('g')
//         .selectAll("dot")
//         .data(data)
//         .enter()
//         .append("circle")
//         .attr("cx", function (d) { console.log("cx", d); })
//         .attr("cy", function (d) { console.log("cy", d); })
//         .attr("r", 1.5)
//         .style("fill", "#69b3a2")
// });
