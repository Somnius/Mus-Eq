// Thin typed wrapper over the MusEq HTTP API. The login cookie is sent automatically (same origin).

export interface Track {
  id: string;
  title: string;
  artist: string | null;
  album: string | null;
  duration_ms: number | null;
  preferred_file_id: string;
}
export interface Skin { id: string; name: string }
export interface SessionSnapshot { state: string; track: Track | null; position_ms: number; volume: number; client_id: string | null }

export interface Station { id: string; name: string; category: string; url: string; alive: boolean | null; error: string | null; checked_at: number | null }

export interface Playlist { id: string; name: string; extend: boolean; track_ids: string[] }
export interface Extension { added: Track[]; pool_empty: boolean; reason?: string }

export interface ToolProgress { phase: string; downloaded: number; total: number; error: string | null }
export interface Tool {
  name: string; purpose: string; version: string; licence: string; homepage: string; notes: string;
  platform_supported: boolean; source_url: string | null; download_bytes: number | null; sha256: string | null;
  found: "installed" | "modified" | "configured" | "system" | "missing"; path: string | null;
  message: string | null; confirm_phrase: string; progress: ToolProgress;
}

export class Unauthorized extends Error {}

async function req(method: string, path: string, body?: unknown, raw?: BodyInit): Promise<Response> {
  const headers: Record<string, string> = {};
  let payload: BodyInit | undefined = raw;
  if (body !== undefined) {
    headers["content-type"] = "application/json";
    payload = JSON.stringify(body);
  } else if (raw !== undefined) {
    headers["content-type"] = "application/octet-stream";
  } else if (method !== "GET") {
    headers["content-type"] = "application/json";
  }
  const res = await fetch(path, { method, headers, body: payload, credentials: "same-origin" });
  if (res.status === 401) throw new Unauthorized();
  return res;
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const msg = await res.json().then((j) => j.error as string).catch(() => res.statusText);
    throw new Error(msg || `HTTP ${res.status}`);
  }
  return res.json() as Promise<T>;
}

export const api = {
  async login(token: string): Promise<boolean> {
    const res = await fetch("/v1/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ token }) });
    return res.status === 204;
  },
  logout: () => req("POST", "/v1/logout"),
  tracks: (q: string) => req("GET", `/v1/tracks?limit=200&q=${encodeURIComponent(q)}`).then((r) => json<{ items: Track[]; total: number }>(r)),
  stations: (q: string, category: string, liveOnly: boolean) =>
    req("GET", `/v1/stations?limit=500&q=${encodeURIComponent(q)}&category=${encodeURIComponent(category)}${liveOnly ? "&alive=true" : ""}`).then((r) => json<{ items: Station[]; total: number }>(r)),
  playlists: () => req("GET", "/v1/playlists").then((r) => json<Playlist[]>(r)),
  playlist: (id: string) => req("GET", `/v1/playlists/${id}`).then((r) => json<Playlist>(r)),
  createPlaylist: (name: string, extend: boolean) => req("POST", "/v1/playlists", { name, extend }).then((r) => json<Playlist>(r)),
  addToPlaylist: async (id: string, trackId: string) => { const r = await req("POST", `/v1/playlists/${id}/items`, { track_id: trackId }); if (!r.ok) throw new Error(`HTTP ${r.status}`); },
  extendPlaylist: (id: string, count: number) => req("POST", `/v1/playlists/${id}/extend`, { count }).then((r) => json<Extension>(r)),
  station: (id: string) => req("GET", `/v1/stations/${id}`).then((r) => json<Station>(r)),
  track: (id: string) => req("GET", `/v1/tracks/${id}`).then((r) => json<Track>(r)),
  checkStations: () => req("POST", "/v1/jobs", { kind: "check_stations" }).then((r) => json<{ id: string }>(r)),
  skins: () => req("GET", "/v1/skins").then((r) => json<Skin[]>(r)),
  uploadSkin: (name: string, file: File) => req("POST", `/v1/skins?name=${encodeURIComponent(name)}`, undefined, file).then((r) => json<Skin>(r)),
  scan: () => req("POST", "/v1/jobs", { kind: "scan" }).then((r) => json<{ id: string }>(r)),
  job: (id: string) => req("GET", `/v1/jobs/${id}`).then((r) => json<{ state: string; result: Record<string, number> | null; error: string | null }>(r)),
  tools: () => req("GET", "/v1/tools").then((r) => json<Tool[]>(r)),
  // the phrase is sent only when the owner clicked the button under the message that names it
  installTool: (name: string, confirm: string) => req("POST", `/v1/tools/${encodeURIComponent(name)}/install`, { confirm }).then((r) => json<Tool>(r)),
  removeTool: async (name: string) => { const r = await req("DELETE", `/v1/tools/${encodeURIComponent(name)}`); if (!r.ok) throw new Error(`HTTP ${r.status}`); },
  report: (body: Record<string, unknown>) => req("POST", "/v1/session", { action: "report", ...body }),
};

export const audioUrl = (t: Track) => `/v1/audio/${t.preferred_file_id}`;
