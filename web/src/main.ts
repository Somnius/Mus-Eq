import Webamp from "webamp";
import type { Track as WebampTrack } from "webamp";
import { Unauthorized, api, audioUrl, type SessionSnapshot, type Track } from "./api";

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
  const report = (force = false) => {
    if (!webamp) return;
    const status = webamp.getMediaStatus();
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
  webamp.__onStateChange(() => report());
}

// ---- remote commands over the WebSocket -------------------------------------------------

function connectEvents() {
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/v1/events`);
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data as string) as { type: string; action?: string; target?: string | null; track_id?: string | null; volume?: number | null; session?: SessionSnapshot };
    if (msg.type !== "command" || msg.target !== clientId || !webamp) return;
    switch (msg.action) {
      case "play": webamp.play(); break;
      case "pause": webamp.pause(); break;
      case "next": webamp.nextTrack(); break;
      case "previous": webamp.previousTrack(); break;
      case "volume": if (typeof msg.volume === "number") webamp.setVolume(Math.round(msg.volume * 100)); break;
    }
  };
  ws.onclose = () => setTimeout(() => { if (!$("app").hidden) connectEvents(); }, 3000);
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
$("logout").addEventListener("click", async () => { await api.logout(); location.reload(); });
$("skin-file").addEventListener("change", async (e) => {
  const f = (e.target as HTMLInputElement).files?.[0];
  if (!f) return;
  try { const s = await api.uploadSkin(f.name, f); await refreshSkins(); webamp?.setSkinFromUrl(`/v1/skins/${s.id}`); }
  catch (err) { $("status").textContent = `Skin upload failed: ${(err as Error).message}`; }
});

void boot();
