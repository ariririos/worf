// import * as d3 from "d3";
// import data from "./data.json";

// const width = 1280;
// const height = 720;

// const x = d3.scaleLinear().domain([0, 1]).range([0, width]).nice();
// const y = d3.scaleLinear().domain([0, 1]).range([0, height]).nice();

// const svg = d3.create("svg").attr("viewBox", [0, 0, width, height]).property("value", []);

// svg.append("g")
//     .attr("transform", `translate(0, ${height})`)
//     .call(d3.axisBottom(x))
//     .call(g => g.select(".domain").remove());

// svg.append("g")
//     .attr("transform", `translate(0, 0)`)
//     .call(d3.axisLeft(y))
//     .call(g => g.select(".domain").remove());

// const dot = svg.append("g")
//     .attr("fill", "none")
//     .attr("stroke", "steelblue")
//     .attr("stroke-width", 1.5)
//     .selectAll("circle")
//     .data(data)
//     .join("circle")
//     .attr("transform", d => `translate(${x(d.x)}, ${y(d.y)})`)
//     .attr("r", 5);

// document.getElementById("container").appendChild(svg.node());