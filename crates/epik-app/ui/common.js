// What the app's pages share: drawing sentences whose issue and pull
// request numbers link to GitHub, and a run's small graph.
"use strict";

const { invoke } = window.__TAURI__.core;
const SVG = "http://www.w3.org/2000/svg";

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

// A link that opens a GitHub page in the browser.
function githubLink(text, url) {
  const a = element("a", "", text);
  a.href = url;
  a.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    invoke("open_github", { url }).catch(() => {});
  });
  return a;
}

// A sentence, from the app's spans: { text, url?, code? }.
function words(spans, className) {
  const sentence = element("span", className);
  for (const span of spans || []) {
    let node;
    if (span.url) node = githubLink(span.text, span.url);
    else if (span.code) node = element("code", "", span.text);
    else node = document.createTextNode(span.text);
    sentence.append(node);
  }
  return sentence;
}

// A run's small graph: its issues as dots by state, joined by their
// dependencies.
function miniGraph(graph) {
  const { layout, states } = graph;
  const step = 16;
  const r = 4;
  const width = Math.max(1, layout.columns) * step;
  const height = Math.max(1, layout.rows) * 12;
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "mini-graph");
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.setAttribute("width", width);
  svg.setAttribute("height", height);
  const at = new Map();
  layout.places.forEach(([number, column, row], i) => {
    at.set(number, { x: column * step + r + 2, y: row * 12 + 6, state: states[i] });
  });
  for (const [from, to] of layout.edges) {
    const a = at.get(from);
    const b = at.get(to);
    const line = document.createElementNS(SVG, "line");
    line.setAttribute("x1", a.x);
    line.setAttribute("y1", a.y);
    line.setAttribute("x2", b.x);
    line.setAttribute("y2", b.y);
    line.setAttribute("class", "mini-edge");
    svg.append(line);
  }
  for (const { x, y, state } of at.values()) {
    const dot = document.createElementNS(SVG, "circle");
    dot.setAttribute("cx", x);
    dot.setAttribute("cy", y);
    dot.setAttribute("r", r);
    dot.setAttribute("class", `mini-node ${state}`);
    svg.append(dot);
  }
  return svg;
}

// A bar of merged, running and not-started issues.
function progressBar(progress) {
  const bar = element("div", "progress");
  const total = progress.merged + progress.running + progress.not_started || 1;
  for (const part of ["merged", "running", "not_started"]) {
    const piece = element("span", part);
    piece.style.flexGrow = progress[part] / total;
    bar.append(piece);
  }
  return bar;
}

// Remembers each running issue's last event, and says whether it changed
// since the last time it was seen, so its card can flash.
const lastEvents = new Map();
function changed(key, lastEvent) {
  const before = lastEvents.get(key);
  lastEvents.set(key, lastEvent);
  return before !== undefined && lastEvent !== null && before !== lastEvent;
}

// Flashes `node` briefly.
function flash(node) {
  node.classList.remove("flash");
  void node.offsetWidth;
  node.classList.add("flash");
}
