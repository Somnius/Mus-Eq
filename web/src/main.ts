// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
import Webamp from "webamp";
import type { Track as WebampTrack } from "webamp";
import { Unauthorized, api, audioUrl, type Playlist, type SessionSnapshot, type Station, type Tool, type Track } from "./api";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const clientId = crypto.randomUUID();
let webamp: Webamp | null = null;
let byUrl = new Map<string, Track>(); // audio url -> server track, to report what is playing

function toWebamp(t: Track): WebampTrack {
  return {
    url: audioUrl(t),
    defaultName: t.title,
    metaData: { artist: t.artist ?? "", title: t.title, album: t.album ?? undefined },
    duration: t.duration_ms ? t.duration_ms / 1000 : undefined,
  };
}
function remember(ts: Track[]) { for (const t of ts) byUrl.set(audioUrl(t), t); }

function showLogin(error = "") {
  $("app").hidden = true;
  $("login").hidden = false;
  $("login-error").textContent = error;
}

async function boot() {
  try {
    await refreshTracks("");
  } catch (e) {
    if (e instanceof Unauthorized) return showLogin();
    $("status").textContent = `Cannot reach the server: ${(e as Error).message}`;
    $("app").hidden = false;
    return;
  }
  $("login").hidden = true;
  $("app").hidden = false;
  await startPlayer();
  connectEvents();
  await refreshSkins();
  await refreshStations();
  await refreshPlaylists();
  await refreshTools();
}

async function refreshTracks(q: string) {
  const { items, total } = await api.tracks(q);
  remember(items);
  $("status").textContent = total === 0 ? "No tracks yet. Put music in the library folder and press Rescan." : `${total} track${total === 1 ? "" : "s"}${total > items.length ? ` (showing ${items.length})` : ""}`;
  const ul = $("tracks");
  ul.replaceChildren(
    ...items.map((t) => {
      const li = document.createElement("li");
      const meta = document.createElement("span");
      meta.className = "meta";
      const title = document.createElement("span");
      title.className = "title";
      title.textContent = t.title;
      const sub = document.createElement("span");
      sub.className = "sub";
      sub.textContent = [t.artist, t.album].filter(Boolean).join(" · ");
      meta.append(title, sub);
      const play = document.createElement("button");
      play.textContent = "Play";
      play.onclick = () => { webamp?.setTracksToPlay([toWebamp(t)]); };
      const add = document.createElement("button");
      add.textContent = "Add";
      add.onclick = () => { webamp?.appendTracks([toWebamp(t)]); };
      const toList = document.createElement("button");
      toList.textContent = "+ Playlist";
      toList.onclick = () => void addToTargetPlaylist(t);
      li.append(meta, play, add, toList);
      return li;
    }),
  );
}

async function refreshSkins() {
  const skins = await api.skins();
  $("skin-list").replaceChildren(
    ...skins.map((s) => {
      const li = document.createElement("li");
      const b = document.createElement("button");
      b.textContent = s.name;
      b.onclick = () => webamp?.setSkinFromUrl(`/v1/skins/${s.id}`);
      li.append(b);
      return li;
    }),
  );
}

// ---- programs Mus-Eq can download (ffmpeg, yt-dlp) --------------------------------------

let toolsTimer: number | undefined;
const busy = (t: Tool) => ["downloading", "verifying", "unpacking", "checking"].includes(t.progress.phase);
const foundText: Record<Tool["found"], string> = {
  installed: "installed by Mus-Eq",
  modified: "installed copy was changed, so it is not used (remove it and install again)",
  configured: "using the program you configured",
  system: "found on this server",
  missing: "not installed",
};

async function refreshTools() {
  let tools: Tool[];
  try { tools = await api.tools(); } catch (e) { $("tools-status").textContent = `Cannot read the programs list: ${(e as Error).message}`; return; }
  $("tools-list").replaceChildren(...tools.map(toolRow));
  window.clearTimeout(toolsTimer);
  // keep the progress current while something downloads
  if (tools.some(busy)) toolsTimer = window.setTimeout(() => void refreshTools(), 1500);
}

function toolRow(t: Tool): HTMLElement {
  const li = document.createElement("li");
  li.className = "tool";
  li.dataset.tool = t.name;
  const head = document.createElement("div");
  head.className = "tool-head";
  const name = document.createElement("strong");
  name.textContent = `${t.name} ${t.version}`;
  const state = document.createElement("span");
  state.className = "sub";
  const pct = t.progress.total > 0 ? Math.floor((100 * t.progress.downloaded) / t.progress.total) : 0;
  state.textContent = busy(t)
    ? `${t.progress.phase}${t.progress.phase === "downloading" ? ` ${pct}%` : "…"}`
    : t.progress.phase === "failed" && t.found === "missing" ? `not installed (last try failed)` : (foundText[t.found] ?? t.found);
  head.append(name, state);
  const why = document.createElement("div");
  why.className = "sub";
  why.textContent = `Used ${t.purpose}.`;
  li.append(head, why);
  if (t.progress.phase === "failed" && t.progress.error) {
    const err = document.createElement("p");
    err.className = "error";
    err.setAttribute("role", "alert");
    err.textContent = t.progress.error;
    li.append(err);
  }
  if (t.found === "installed" && !busy(t)) {
    const rm = document.createElement("button");
    rm.textContent = "Remove";
    rm.onclick = async () => { try { await api.removeTool(t.name); } catch (e) { $("tools-status").textContent = (e as Error).message; } await refreshTools(); };
    li.append(rm);
  } else if (t.found === "missing" && !busy(t)) {
    if (!t.platform_supported || !t.message) {
      const no = document.createElement("p");
      no.className = "sub";
      no.textContent = "This release has no download for this server's system. Install it yourself and set its environment variable (for example MUSEQ_FFMPEG).";
      li.append(no);
    } else {
      // the full message is shown first; the confirmation is sent only by the second button
      const details = document.createElement("details");
      const sum = document.createElement("summary");
      sum.textContent = "What would be downloaded?";
      const msg = document.createElement("p");
      msg.className = "tool-msg";
      msg.textContent = t.message;
      const go = document.createElement("button");
      go.textContent = "I understand, download and install";
      go.onclick = async () => {
        go.disabled = true;
        try { await api.installTool(t.name, t.confirm_phrase); } catch (e) { $("tools-status").textContent = (e as Error).message; }
        await refreshTools();
      };
      details.append(sum, msg, go);
      li.append(details);
    }
  }
  return li;
}

// ---- player + session reporting -------------------------------------------------------

async function startPlayer() {
  if (webamp) return;
  const skins = await api.skins().catch(() => []);
  webamp = new Webamp({
    availableSkins: skins.map((s) => ({ url: `/v1/skins/${s.id}`, name: s.name })),
    enableMediaSession: true,
  });
  await webamp.renderWhenReady($("webamp"));
  let lastKey = "";
  let lastSent = 0;
  let currentTrackId: string | null = null;
  let lastWebampStatus = "";
  let lastPlayerStatus = "";
  const report = (force = false) => {
    if (!webamp) return;
    const status = webamp.getMediaStatus();
    // Webamp just started playing: the station stops (one source at a time)
    if (status === "PLAYING" && lastWebampStatus !== "PLAYING" && !stAudio().paused) stAudio().pause();
    lastWebampStatus = status;
    const ps = webamp.getPlayerMediaStatus();
    if (ps === "ENDED" && lastPlayerStatus !== "ENDED") void extendActivePlaylist();
    lastPlayerStatus = ps;
    const state = status === "PLAYING" ? "playing" : status === "PAUSED" ? "paused" : "stopped";
    const key = `${state}|${currentTrackId ?? ""}`;
    const now = Date.now();
    // report on change, and at most every 10 s otherwise
    if (!force && key === lastKey && now - lastSent < 10_000) return;
    lastKey = key;
    lastSent = now;
    void api.report({ client_id: clientId, state, track_id: currentTrackId ?? undefined });
  };
  webamp.onTrackDidChange((t) => {
    currentTrackId = t ? (byUrl.get(new URL(t.url, location.href).pathname)?.id ?? null) : null;
    report(true);
  });
  // __onStateChange is a private Webamp hook; if an upgrade removes it, reporting stops (see DECISIONS).
  webamp.__onStateChange(() => report());
  // Also re-report every 10 s while playing, so a restarted server learns the session again.
  window.setInterval(() => { if (webamp?.getMediaStatus() === "PLAYING") report(true); }, 10_000);
}

// ---- remote commands over the WebSocket -------------------------------------------------

function connectEvents() {
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/v1/events?client_id=${clientId}`);
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data as string) as { type: string; action?: string; target?: string | null; track_id?: string | null; station_id?: string | null; volume?: number | null; session?: SessionSnapshot };
    if (msg.type !== "command" || msg.target !== clientId || !webamp) return;
    // `play` that names something to play: start that station or track
    if (msg.action === "play" && msg.station_id) {
      void api.station(msg.station_id).then((st) => playStation(st)).catch(() => undefined);
      return;
    }
    if (msg.action === "play" && msg.track_id) {
      void api.track(msg.track_id).then((t) => { remember([t]); stAudio().pause(); webamp?.setTracksToPlay([toWebamp(t)]); }).catch(() => undefined);
      return;
    }
    // a station is the active source if it is playing, or paused after having played while Webamp is not
    const stationActive = currentStation !== null && (!stAudio().paused || (stAudio().currentTime > 0 && webamp.getMediaStatus() !== "PLAYING"));
    if (stationActive) {
      const a = stAudio();
      if (msg.action === "play") void a.play();
      else if (msg.action === "pause") a.pause();
      else if (msg.action === "volume" && typeof msg.volume === "number") a.volume = msg.volume;
      return;
    }
    switch (msg.action) {
      case "play": webampPlay(); break;
      case "pause": webampPause(); break;
      case "next": webamp.nextTrack(); break;
      case "previous": webamp.previousTrack(); break;
      case "volume": if (typeof msg.volume === "number") webamp.setVolume(Math.round(msg.volume * 100)); break;
    }
  };
  ws.onclose = () => setTimeout(() => { if (!$("app").hidden) connectEvents(); }, 3000);
}

// ---- playlists ---------------------------------------------------------------------------------
// Playing a playlist hands its tracks to Webamp. When Webamp reaches the end of a playlist that
// has "extend" switched on, the server picks new tracks from the library (same genre, artists not
// already in the list) and they are appended and played.

let playlists: Playlist[] = [];
let activePlaylist: { id: string; name: string; extend: boolean } | null = null;

async function refreshPlaylists() {
  playlists = await api.playlists();
  const target = $("pl-target") as HTMLSelectElement;
  const keep = target.value;
  target.replaceChildren(...playlists.map((p) => { const o = document.createElement("option"); o.value = p.id; o.textContent = p.name; return o; }));
  if (keep && playlists.some((p) => p.id === keep)) target.value = keep;
  $("pl-list").replaceChildren(...playlists.map((p) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = `${p.name} (${p.track_ids.length}${p.extend ? ", extends" : ""})`;
    const play = document.createElement("button");
    play.textContent = "Play";
    play.disabled = p.track_ids.length === 0;
    play.onclick = () => void playPlaylist(p.id);
    li.append(name, play);
    return li;
  }));
}

async function addToTargetPlaylist(t: Track) {
  const id = ($("pl-target") as HTMLSelectElement).value;
  if (!id) { $("pl-status").textContent = "Create a playlist first."; return; }
  try { await api.addToPlaylist(id, t.id); $("pl-status").textContent = `Added "${t.title}".`; await refreshPlaylists(); }
  catch (e) { $("pl-status").textContent = `Could not add: ${(e as Error).message}`; }
}

async function playPlaylist(id: string) {
  const p = await api.playlist(id);
  const tracks = await Promise.all(p.track_ids.map((tid) => api.track(tid)));
  remember(tracks);
  stAudio().pause();
  activePlaylist = { id: p.id, name: p.name, extend: p.extend };
  webamp?.setTracksToPlay(tracks.map(toWebamp));
  $("pl-status").textContent = `Playing "${p.name}"${p.extend ? "; it will add new artists when it ends" : ""}.`;
}

async function extendActivePlaylist() {
  if (!activePlaylist?.extend) return;
  const { id, name } = activePlaylist;
  try {
    const r = await api.extendPlaylist(id, 3);
    if (r.added.length === 0) { $("pl-status").textContent = `"${name}" ended. ${r.reason ?? "Nothing new to add."}`; return; }
    remember(r.added);
    // after the end, "next" does not restart the player: select the first new track and play it
    const firstNew = webamp?.getPlaylistTracks().length ?? 0;
    webamp?.appendTracks(r.added.map(toWebamp));
    webamp?.setCurrentTrack(firstNew);
    webampPlay();
    $("pl-status").textContent = `"${name}" ended: added ${r.added.length} new track${r.added.length === 1 ? "" : "s"} by ${r.added.map((t) => t.artist ?? "?").join(", ")}.`;
    await refreshPlaylists();
  } catch (e) { $("pl-status").textContent = `Could not extend "${name}": ${(e as Error).message}`; }
}

// ---- radio stations ------------------------------------------------------------------------
// External stations are played by the browser straight from the station (the server never relays
// them). A plain <audio> element is used because most streams don't send CORS headers, which
// Webamp's loader needs.

const stAudio = () => $("st-audio") as HTMLAudioElement;
let currentStation: Station | null = null;

function reportStation() {
  const a = stAudio();
  if (!currentStation) return;
  const state = a.paused ? (a.currentTime > 0 ? "paused" : "stopped") : "playing";
  void api.report({ client_id: clientId, state, station_id: currentStation.id });
}

// Webamp's pause()/play() press the player's buttons: pause toggles and play restarts, so only
// press them when the state says it matters.
function webampPause() { if (webamp?.getMediaStatus() === "PLAYING") webamp.pause(); }
function webampPlay() { if (webamp && webamp.getMediaStatus() !== "PLAYING") webamp.play(); }

function playStation(s: Station) {
  const a = stAudio();
  webampPause();
  currentStation = s;
  a.src = s.url;
  $("st-status").textContent = `Connecting to ${s.name}…`;
  a.play().catch((e: Error) => { $("st-status").textContent = `Could not play ${s.name}: ${e.message}`; });
}

for (const ev of ["playing", "pause", "ended"]) stAudio().addEventListener(ev, () => {
  if (ev === "playing" && currentStation) $("st-status").textContent = `Playing ${currentStation.name}`;
  reportStation();
});
stAudio().addEventListener("error", () => {
  if (currentStation) $("st-status").textContent = `${currentStation.name} could not be played in the browser (the stream may be down, or blocked as mixed content on an HTTPS page).`;
});

async function refreshStations() {
  const q = ($("st-search") as HTMLInputElement).value;
  const cat = ($("st-cat") as HTMLSelectElement).value;
  const live = ($("st-live") as HTMLInputElement).checked;
  const { items, total } = await api.stations(q, cat, live);
  if (!currentStation) $("st-status").textContent = total === 0 ? "No stations match." : `${total} station${total === 1 ? "" : "s"}`;
  $("st-list").replaceChildren(...items.map((s) => {
    const li = document.createElement("li");
    const dot = document.createElement("span");
    dot.className = `dot ${s.alive === true ? "live" : s.alive === false ? "dead" : ""}`;
    dot.title = s.alive === null ? "not checked yet" : s.alive ? "stream answered" : (s.error ?? "stream is down");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = s.name;
    const play = document.createElement("button");
    play.textContent = "Play";
    const blocked = location.protocol === "https:" && s.url.startsWith("http:");
    play.disabled = blocked;
    if (blocked) play.title = "Plain-HTTP streams are blocked on HTTPS pages";
    play.onclick = () => playStation(s);
    li.append(dot, name, play);
    return li;
  }));
}

// ---- UI wiring ----------------------------------------------------------------------------

$("login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const ok = await api.login(($("token") as HTMLInputElement).value.trim());
  if (ok) { ($("token") as HTMLInputElement).value = ""; await boot(); } else showLogin("That token was not accepted.");
});
let searchTimer: number | undefined;
$("search").addEventListener("input", (e) => {
  clearTimeout(searchTimer);
  const q = (e.target as HTMLInputElement).value;
  searchTimer = window.setTimeout(() => void refreshTracks(q), 200);
});
$("scan").addEventListener("click", async () => {
  $("status").textContent = "Scanning…";
  const { id } = await api.scan().catch((e: Error) => { $("status").textContent = e.message; return { id: "" }; });
  for (let i = 0; id && i < 600; i++) {
    const j = await api.job(id);
    if (j.state === "done" || j.state === "failed") { $("status").textContent = j.state === "done" ? "Scan finished." : `Scan failed: ${j.error ?? ""}`; break; }
    await new Promise((r) => setTimeout(r, 500));
  }
  await refreshTracks(($("search") as HTMLInputElement).value);
});
let stTimer: number | undefined;
const stChanged = () => { clearTimeout(stTimer); stTimer = window.setTimeout(() => void refreshStations(), 200); };
$("st-search").addEventListener("input", stChanged);
$("st-cat").addEventListener("change", stChanged);
$("st-live").addEventListener("change", stChanged);
$("st-check").addEventListener("click", async () => {
  $("st-status").textContent = "Checking streams…";
  const { id } = await api.checkStations().catch((e: Error) => { $("st-status").textContent = e.message; return { id: "" }; });
  for (let i = 0; id && i < 240; i++) {
    const j = await api.job(id);
    if (j.state === "done" || j.state === "failed") {
      $("st-status").textContent = j.state === "done" ? `Checked ${j.result?.checked ?? "?"} streams: ${j.result?.alive ?? "?"} answered.` : `Check failed: ${j.error ?? ""}`;
      break;
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  await refreshStations();
});
$("pl-create").addEventListener("click", async () => {
  const input = $("pl-name") as HTMLInputElement;
  const name = input.value.trim();
  if (!name) { $("pl-status").textContent = "Give the playlist a name."; return; }
  try { const p = await api.createPlaylist(name, ($("pl-extend") as HTMLInputElement).checked); input.value = ""; await refreshPlaylists(); ($("pl-target") as HTMLSelectElement).value = p.id; $("pl-status").textContent = `Created "${p.name}".`; }
  catch (e) { $("pl-status").textContent = `Could not create: ${(e as Error).message}`; }
});
$("logout").addEventListener("click", async () => { await api.logout(); location.reload(); });
$("skin-file").addEventListener("change", async (e) => {
  const f = (e.target as HTMLInputElement).files?.[0];
  if (!f) return;
  try { const s = await api.uploadSkin(f.name, f); await refreshSkins(); webamp?.setSkinFromUrl(`/v1/skins/${s.id}`); }
  catch (err) { $("status").textContent = `Skin upload failed: ${(err as Error).message}`; }
});

void boot();
