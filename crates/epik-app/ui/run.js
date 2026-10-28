// Draws a run's view, which the app lays out, and asks for it again every
// second. The window only shows the run: nothing here changes anything.
"use strict";

const NODE_WIDTH = 220;
const NODE_HEIGHT = 72;
const COLUMN_GAP = 64;
const ROW_GAP = 20;
const SVG = "http://www.w3.org/2000/svg";

const { invoke } = window.__TAURI__.core;
const runId = window.EPIK_RUN_ID;

function text(id, value) {
  document.getElementById(id).textContent = value ?? "";
}

function x(column) {
  return column * (NODE_WIDTH + COLUMN_GAP);
}

function y(row) {
  return row * (NODE_HEIGHT + ROW_GAP);
}

function details(node) {
  return [
    node.pull_request && `PR #${node.pull_request}`,
    node.duration,
    node.cost,
  ]
    .filter(Boolean)
    .join(" · ");
}

function drawHeader(view) {
  document.body.dataset.status = view.status;
  text("repo", `${view.repo}#${view.number}`);
  text("title", view.title);
  const status = document.getElementById("status");
  status.textContent = view.pull_request
    ? `${view.status} (PR #${view.pull_request})`
    : view.status;
  status.className = `status ${view.status}`;
  text("elapsed", view.elapsed);
  text("cost", view.cost);
  text("reason", view.reason);
  document.title = `${view.repo}#${view.number} ${view.title}`;
}

function drawNodes(view) {
  const nodes = document.getElementById("nodes");
  nodes.replaceChildren(
    ...view.nodes.map((node) => {
      const box = document.createElement("div");
      box.className = `node ${node.status}`;
      box.style.left = `${x(node.column)}px`;
      box.style.top = `${y(node.row)}px`;
      box.style.width = `${NODE_WIDTH}px`;
      box.style.height = `${NODE_HEIGHT}px`;
      box.title = [`#${node.number} ${node.title}`, node.status, node.reason]
        .filter(Boolean)
        .join("\n");
      for (const [name, value] of [
        ["number", `#${node.number}`],
        ["name", node.title],
        ["details", details(node) || node.status],
      ]) {
        const line = document.createElement("div");
        line.className = name;
        line.textContent = value;
        box.append(line);
      }
      return box;
    }),
  );
}

function drawEdges(view) {
  const at = new Map(view.nodes.map((node) => [node.number, node]));
  const svg = document.getElementById("edges");
  svg.querySelectorAll("path.edge").forEach((path) => path.remove());
  for (const edge of view.edges) {
    const from = at.get(edge.from);
    const to = at.get(edge.to);
    const x1 = x(from.column) + NODE_WIDTH;
    const y1 = y(from.row) + NODE_HEIGHT / 2;
    const x2 = x(to.column);
    const y2 = y(to.row) + NODE_HEIGHT / 2;
    const bend = Math.max(COLUMN_GAP, (x2 - x1) / 2);
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("class", "edge");
    path.setAttribute(
      "d",
      `M${x1},${y1} C${x1 + bend},${y1} ${x2 - bend},${y2} ${x2},${y2}`,
    );
    path.setAttribute("marker-end", "url(#arrow)");
    svg.append(path);
  }
}

function draw(view) {
  drawHeader(view);
  const graph = document.getElementById("graph");
  const width = Math.max(0, x(view.columns) - COLUMN_GAP);
  const height = Math.max(0, y(view.rows) - ROW_GAP);
  graph.style.width = `${width}px`;
  graph.style.height = `${height}px`;
  const svg = document.getElementById("edges");
  svg.setAttribute("width", width);
  svg.setAttribute("height", height);
  drawNodes(view);
  drawEdges(view);
}

async function refresh() {
  try {
    draw(await invoke("run_view", { runId }));
    text("error", "");
  } catch (error) {
    text("error", `Couldn't read the run: ${error}`);
  }
}

refresh();
setInterval(refresh, 1000);
