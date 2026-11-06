import Webamp from "webamp";
import type { Track as WebampTrack } from "webamp";
import { Unauthorized, api, audioUrl, type SessionSnapshot, type Station, type Track } from "./api";

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
      li.append(meta, play, add);
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
  const report = (force = false) => {
    if (!webamp) return;
    const status = webamp.getMediaStatus();
    // Webamp just started playing: the station stops (one source at a time)
    if (status === "PLAYING" && lastWebampStatus !== "PLAYING" && !stAudio().paused) stAudio().pause();
    lastWebampStatus = status;
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
$("logout").addEventListener("click", async () => { await api.logout(); location.reload(); });
$("skin-file").addEventListener("change", async (e) => {
  const f = (e.target as HTMLInputElement).files?.[0];
  if (!f) return;
  try { const s = await api.uploadSkin(f.name, f); await refreshSkins(); webamp?.setSkinFromUrl(`/v1/skins/${s.id}`); }
  catch (err) { $("status").textContent = `Skin upload failed: ${(err as Error).message}`; }
});

void boot();
