// The main window. It asks the app what to show every second and draws it:
// Home (#/), a run's page (#/run/<owner/name#n>), and Settings over Home.
// Its only controls are Rerun and Delete on a run that needs attention,
// and Settings.
"use strict";

const REFRESH_MS = 1000;
const MARK = `<svg viewBox="0 0 64 64" width="20" height="20" aria-hidden="true">
  <g stroke="#171717" stroke-width="3" opacity="0.3"><line x1="16" y1="16" x2="48" y2="24"/><line x1="16" y1="16" x2="24" y2="48"/><line x1="48" y1="24" x2="40" y2="48"/><line x1="24" y1="48" x2="40" y2="48"/></g>
  <circle cx="16" cy="16" r="7" fill="#00c07a"/><circle cx="48" cy="24" r="6" fill="#171717"/><circle cx="24" cy="48" r="6" fill="#171717"/><circle cx="40" cy="48" r="5" fill="#00c07a" opacity="0.7"/></svg>`;

// The issue selected on the run page, by run.
const selected = new Map();
let error = "";

// A redraw replaces what it draws, so none happens while a mouse button is
// down: a click whose press and release straddled one would be lost.
let pressing = false;
document.addEventListener("mousedown", () => (pressing = true));
document.addEventListener("mouseup", () => setTimeout(() => (pressing = false)));

// The bar as last drawn, so it is redrawn only when it changes.
let barDrawn = "";
function drawBar(key, draw) {
  if (key === barDrawn) return;
  barDrawn = key;
  draw();
}

function route() {
  const hash = decodeURIComponent(window.location.hash.replace(/^#\/?/, ""));
  if (hash.startsWith("run/")) return { page: "run", run: hash.slice(4) };
  return { page: "home" };
}

// The hash for a route. A run's `#` would end the URL's fragment.
function href(path) {
  return `#/${path.replaceAll("#", "%23")}`;
}

function go(path) {
  window.location.hash = href(path);
}

function mark() {
  const span = element("span", "mark");
  span.innerHTML = MARK;
  return span;
}

// --- Home --------------------------------------------------------------

function drawHomeBar(home) {
  drawBar(`home ${home.headline} ${home.missing_repository}`, () => drawHomeBarNow(home));
}

function drawHomeBarNow(home) {
  const bar = document.getElementById("bar");
  const gear = element("button", "gear", "⚙");
  gear.title = "Settings";
  gear.setAttribute("aria-label", "Settings");
  if (home.missing_repository) gear.append(element("span", "badge", "✕"));
  gear.addEventListener("click", openSettings);
  bar.replaceChildren(mark(), element("span", "name", "Epik"), element("span", "headline", home.headline), gear);
}

function figures(card) {
  const box = element("div", "figures");
  box.append(element("span", "time", card.elapsed), element("span", "cost", card.cost));
  return box;
}

function rowsOf(card) {
  const rows = element("div", "rows");
  const row = (label, content, extra) => {
    const line = element("div", "line");
    line.append(element("span", "label", label), content);
    if (extra) line.append(extra);
    return line;
  };
  for (const now of card.now) {
    const life = element("span", `life ${now.life?.startsWith("quiet") ? "quiet" : ""}`, now.life);
    const line = row("Now", words(now.words), life);
    if (changed(`${card.run}#${now.number}`, now.last_event)) flash(line);
    rows.append(line);
  }
  if (card.next) rows.append(row("Next", words(card.next)));
  if (card.done) rows.append(row("Done", words(card.done)));
  return rows;
}

function buildingCard(card) {
  const box = element("article", "card building");
  const head = element("div", "card-head");
  const titles = element("div", "titles");
  const title = element("a", "title", card.title);
  title.href = href(`run/${card.run}`);
  const sub = element("div", "sub");
  sub.append(githubLink(card.epic.text, card.epic.url), document.createTextNode(` · ${card.count}`));
  titles.append(title, sub);
  const open = element("a", "open", "Open ›");
  open.href = title.href;
  const right = figures(card);
  right.append(open);
  head.append(miniGraph(card.graph), titles, right);
  box.append(head, progressBar(card.progress), rowsOf(card));
  return box;
}

function endedCard(card, attention) {
  const box = element("article", `card ${attention ? "attention" : "waiting"}`);
  const head = element("div", "card-head");
  const titles = element("div", "titles");
  const title = element("a", "title", card.title);
  title.href = href(`run/${card.run}`);
  const sub = element("div", attention ? "sub said bad" : "sub said good");
  sub.append(githubLink(card.epic.text, card.epic.url), document.createTextNode(" · "), words(card.sentence));
  titles.append(title, sub);
  if (card.note) titles.append(element("div", "note", card.note));
  const right = figures(card);
  if (attention) {
    const rerun = element("button", "primary", "Rerun");
    rerun.addEventListener("click", () => act("rerun", card.run));
    const remove = element("button", "", "Delete");
    remove.addEventListener("click", () => act("delete_run", card.run));
    right.append(rerun, remove);
  }
  head.append(miniGraph(card.graph), titles, right);
  box.append(head);
  return box;
}

async function act(command, run) {
  try {
    await invoke(command, { run });
    error = "";
  } catch (reason) {
    error = String(reason);
  }
  refresh();
}

function section(title, cards, draw, className) {
  if (cards.length === 0) return [];
  return [element("h2", `section ${className || ""}`, title), ...cards.map(draw)];
}

function drawHome(home) {
  drawHomeBar(home);
  const page = document.getElementById("page");
  const parts = [
    ...section("Building", home.building, buildingCard),
    ...section("Waiting for your review", home.waiting, (c) => endedCard(c, false)),
    ...section("Needs attention", home.attention, (c) => endedCard(c, true), "bad"),
  ];
  if (parts.length === 0) {
    const empty = element("div", "empty");
    empty.append(
      element("p", "", "No runs right now."),
      element("p", "muted", "Ask Epik to build an issue or an epic in Claude Code, and it shows up here while it builds."),
    );
    parts.push(empty);
  }
  if (error) parts.unshift(element("p", "error", error));
  page.replaceChildren(...parts);
}

// --- A run's page ----------------------------------------------------------

const CARD_W = 200;
const CARD_H = 76;
const GAP_X = 56;
const GAP_Y = 18;

function drawRunBar(page) {
  drawBar(`run ${page.title} ${JSON.stringify(page.also_building)}`, () => drawRunBarNow(page));
}

function drawRunBarNow(page) {
  const bar = document.getElementById("bar");
  const back = element("a", "back", "‹ Runs");
  back.href = "#/";
  const crumbs = element("span", "crumbs");
  crumbs.append(back, element("span", "slash", "/"), element("span", "", page.title));
  const parts = [mark(), crumbs];
  if (page.also_building.length) {
    const also = element("span", "also", "Also building: ");
    page.also_building.forEach(([run, title], i) => {
      if (i) also.append(document.createTextNode(", "));
      const a = element("a", "", title);
      a.href = href(`run/${run}`);
      also.append(a);
    });
    parts.push(also);
  }
  bar.replaceChildren(...parts);
}

function issueCard(page, issue, chosen) {
  const card = element("button", `issue ${issue.state}${chosen ? " chosen" : ""}`);
  card.style.left = `${issue.column * (CARD_W + GAP_X)}px`;
  card.style.top = `${issue.row * (CARD_H + GAP_Y)}px`;
  card.style.width = `${CARD_W}px`;
  card.style.height = `${CARD_H}px`;
  const top = element("div", "top");
  top.append(element("span", "dot"), githubLink(`#${issue.number}`, issue.url));
  if (issue.life) top.append(element("span", `life ${issue.life.startsWith("quiet") ? "quiet" : ""}`, issue.life));
  else if (issue.figures) top.append(element("span", "figures-small", issue.figures));
  card.append(top, element("div", "name", issue.title), words(issue.line, "card-line"));
  card.title = `#${issue.number} ${issue.title}`;
  card.addEventListener("click", () => {
    selected.set(page.run, issue.number);
    refresh();
  });
  if (changed(`${page.run}#${issue.number}`, issue.last_event)) flash(card);
  return card;
}

function edges(page, svg) {
  const at = new Map(page.issues.map((issue) => [issue.number, issue]));
  for (const [from, to] of page.edges) {
    const a = at.get(from);
    const b = at.get(to);
    const x1 = a.column * (CARD_W + GAP_X) + CARD_W;
    const y1 = a.row * (CARD_H + GAP_Y) + CARD_H / 2;
    const x2 = b.column * (CARD_W + GAP_X);
    const y2 = b.row * (CARD_H + GAP_Y) + CARD_H / 2;
    const bend = Math.max(GAP_X / 2, (x2 - x1) / 2);
    const path = document.createElementNS(SVG, "path");
    path.setAttribute("class", "edge");
    path.setAttribute("d", `M${x1},${y1} C${x1 + bend},${y1} ${x2 - bend},${y2} ${x2},${y2}`);
    svg.append(path);
  }
}

function detail(page, issue) {
  const panel = element("section", "detail");
  const left = element("div", "facts");
  const heading = element("h3");
  heading.append(githubLink(`#${issue.number}`, issue.url), document.createTextNode(` ${issue.title}`));
  left.append(heading, words(issue.sentence, `state ${issue.state}`));
  const facts = element("dl");
  const fact = (name, value) => {
    facts.append(element("dt", "", name));
    const dd = element("dd");
    dd.append(typeof value === "string" ? document.createTextNode(value) : value);
    facts.append(dd);
  };
  if (issue.worked) fact(issue.state === "running" ? "Working for" : "Took", issue.worked);
  fact("Depends on", words(issue.depends_on));
  fact("Unblocks", words(issue.unblocks));
  fact("Pull request", words(issue.pull_request));
  left.append(facts);
  const right = element("div", "doing");
  const head = element("div", "doing-head");
  head.append(element("span", "label", "What the agent has done"));
  if (issue.log) {
    const full = element("a", "full-log", "Full log");
    full.href = "#";
    full.addEventListener("click", (event) => {
      event.preventDefault();
      invoke("open_log", { path: issue.log }).catch((reason) => {
        error = String(reason);
      });
    });
    head.append(full);
  }
  const list = element("ol", "recent");
  if (issue.recent.length === 0) list.append(element("li", "muted", "Nothing yet."));
  for (const action of issue.recent) list.append(element("li", "", action));
  right.append(head, list);
  panel.append(left, right);
  return panel;
}

function drawRun(page) {
  drawRunBar(page);
  const head = element("section", "run-head");
  const about = element("div", "about");
  about.append(githubLink(page.epic.text, page.epic.url), document.createTextNode(" · "), words(page.about));
  const left = element("div");
  left.append(about, element("h1", "", page.title), element("p", `sentence ${page.disposition}`, page.sentence));
  const right = element("dl", "totals");
  right.append(element("dt", "", "Elapsed"), element("dd", "", page.elapsed), element("dt", "", "Cost so far"), element("dd", "", page.cost));
  head.append(left, right);

  const builds = element("div", "builds");
  builds.append(element("span", "label", "Builds"));
  page.builds.forEach((sentence, i) => builds.append(element("span", i === 0 ? "build current" : "build", sentence)));

  const chosenNumber = selected.get(page.run) ?? (page.issues.find((i) => i.state === "running") || page.issues[0])?.number;
  const graph = element("div", "graph");
  const width = Math.max(0, page.columns * (CARD_W + GAP_X) - GAP_X);
  const height = Math.max(0, page.rows * (CARD_H + GAP_Y) - GAP_Y);
  graph.style.width = `${width}px`;
  graph.style.height = `${height}px`;
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("width", width);
  svg.setAttribute("height", height);
  edges(page, svg);
  graph.append(svg, ...page.issues.map((issue) => issueCard(page, issue, issue.number === chosenNumber)));
  const scroller = element("div", "graph-scroll");
  scroller.append(graph);

  const parts = [head, builds, scroller];
  const chosen = page.issues.find((issue) => issue.number === chosenNumber);
  if (chosen) parts.push(detail(page, chosen));
  if (error) parts.unshift(element("p", "error", error));
  // Redrawn every second: keep where the graph was scrolled to.
  const before = document.querySelector(".graph-scroll");
  const [scrolledLeft, scrolledTop] = before ? [before.scrollLeft, before.scrollTop] : [0, 0];
  document.getElementById("page").replaceChildren(...parts);
  scroller.scrollLeft = scrolledLeft;
  scroller.scrollTop = scrolledTop;
}

// --- Settings ----------------------------------------------------------

async function openSettings() {
  await drawSettings();
  document.getElementById("settings").showModal();
}

async function drawSettings() {
  const list = document.getElementById("repositories");
  let settings;
  try {
    settings = await invoke("settings");
  } catch (reason) {
    list.replaceChildren(element("li", "error", String(reason)));
    return;
  }
  list.replaceChildren(
    ...settings.repositories.map((repository) => {
      const item = element("li", repository.missing ? "repository missing" : "repository");
      item.append(element("span", repository.missing ? "status-x" : "status-dot", repository.missing ? "✕" : ""));
      const text = element("div", "repo-text");
      text.append(githubLink(repository.repo, repository.url));
      text.append(
        element(
          "div",
          repository.missing ? "why" : "muted",
          repository.missing ? "No longer on GitHub. It was deleted, renamed, or you lost access." : repository.about,
        ),
      );
      const button = element("button", "", repository.missing ? "Remove" : "Stop monitoring");
      button.addEventListener("click", () => (repository.missing ? stopMonitoring(repository.repo) : confirmStop(repository)));
      item.append(text, button);
      return item;
    }),
  );
  if (settings.repositories.length === 0) list.append(element("li", "muted none", "No repositories yet."));
}

function confirmStop(repository) {
  const dialog = document.getElementById("confirm");
  document.getElementById("confirm-title").textContent = `Stop monitoring ${repository.repo}?`;
  const runs = repository.runs;
  document.getElementById("confirm-text").textContent = runs.length
    ? `Epik will stop checking it, and ${runs.length === 1 ? "this run" : `these ${runs.length} runs`} will disappear from the window:`
    : "Epik will stop checking it. It has no runs in the window.";
  document.getElementById("confirm-runs").replaceChildren(...runs.map(([run, title]) => element("li", "", `${title} (${run})`)));
  dialog.returnValue = "";
  dialog.showModal();
  document.getElementById("confirm-cancel").focus();
  dialog.addEventListener(
    "close",
    () => {
      if (dialog.returnValue === "ok") stopMonitoring(repository.repo);
    },
    { once: true },
  );
}

async function stopMonitoring(repo) {
  try {
    await invoke("stop_monitoring", { repo });
  } catch (reason) {
    error = String(reason);
  }
  await drawSettings();
  refresh();
}

document.getElementById("add").addEventListener("submit", async (event) => {
  event.preventDefault();
  const input = document.getElementById("repo");
  const message = document.getElementById("add-error");
  try {
    await invoke("monitor", { repo: input.value });
    input.value = "";
    message.textContent = "";
  } catch (reason) {
    message.textContent = String(reason);
  }
  await drawSettings();
});

// --- Refreshing ----------------------------------------------------------

let refreshing = false;

async function refresh() {
  if (refreshing || pressing) return;
  refreshing = true;
  try {
    const where = route();
    if (where.page === "run") {
      const page = await invoke("run_page", { run: where.run });
      document.title = `${page.title} · Epik`;
      drawRun(page);
    } else {
      document.title = "Epik";
      drawHome(await invoke("home"));
    }
  } catch (reason) {
    if (route().page === "run" && String(reason).startsWith("no run")) {
      go("");
    } else {
      document.getElementById("page").replaceChildren(element("p", "error", `Couldn't read the runs: ${reason}`));
    }
  } finally {
    refreshing = false;
  }
}

window.addEventListener("hashchange", () => {
  error = "";
  refresh();
});
refresh();
setInterval(refresh, REFRESH_MS);
