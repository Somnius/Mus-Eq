// Headless smoke test of the built client against a real server.
// Usage: MUSEQ_URL=http://127.0.0.1:8099 MUSEQ_TOKEN=... node e2e/smoke.mjs   (CHROMIUM=/usr/bin/chromium)
import { chromium } from "playwright-core";
import http from "node:http";

// A fake radio station: a 30 s 8 kHz mono WAV tone, so the browser has a real stream to play.
function wav() {
  const rate = 8000, n = rate * 30, data = Buffer.alloc(n * 2);
  for (let i = 0; i < n; i++) data.writeInt16LE(Math.round(8000 * Math.sin((2 * Math.PI * 440 * i) / rate)), i * 2);
  const h = Buffer.alloc(44);
  h.write("RIFF", 0); h.writeUInt32LE(36 + data.length, 4); h.write("WAVEfmt ", 8); h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20); h.writeUInt16LE(1, 22); h.writeUInt32LE(rate, 24); h.writeUInt32LE(rate * 2, 28);
  h.writeUInt16LE(2, 32); h.writeUInt16LE(16, 34); h.write("data", 36); h.writeUInt32LE(data.length, 40);
  return Buffer.concat([h, data]);
}
const tone = wav();
const radio = http.createServer((req, res) => {
  res.writeHead(200, { "content-type": "audio/wav", "content-length": tone.length, "accept-ranges": "none" });
  res.end(tone);
});
await new Promise((r) => radio.listen(0, "127.0.0.1", r));
const stationUrl = `http://127.0.0.1:${radio.address().port}/live`;

const base = process.env.MUSEQ_URL;
const token = process.env.MUSEQ_TOKEN;
if (!base || !token) throw new Error("set MUSEQ_URL and MUSEQ_TOKEN");
const fails = [];
const check = (ok, what) => { console.log(`${ok ? "ok  " : "FAIL"} ${what}`); if (!ok) fails.push(what); };

const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM ?? "/usr/bin/chromium",
  args: ["--autoplay-policy=no-user-gesture-required", "--no-sandbox"],
});
const page = await (await browser.newContext()).newPage();
const errors = [];
// Diagnostics: remember who paused a media element, to explain a failed playback check.
await page.addInitScript(() => {
  window.__pauses = [];
  const orig = HTMLMediaElement.prototype.pause;
  HTMLMediaElement.prototype.pause = function () { window.__pauses.push((this.id || "?") + ": " + new Error().stack.split("\n").slice(2, 5).join(" <- ")); return orig.apply(this, arguments); };
});
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => { if (m.type() === "error") errors.push(`console: ${m.text()}`); });

await page.goto(base);
await page.waitForSelector("#login-form", { state: "visible" });
check(await page.isHidden("#app"), "login form shown, app hidden before signing in");

await page.fill("#token", "definitely-not-the-token");
await page.click("#login-form button");
await page.waitForFunction(() => document.getElementById("login-error").textContent.length > 0);
check(true, "wrong token shows an error");

await page.fill("#token", token);
await page.click("#login-form button");
await page.waitForSelector("#app", { state: "visible" });
await page.waitForFunction(() => document.querySelectorAll("#tracks li").length > 0);
const n = await page.locator("#tracks li").count();
check(n >= 1, `library lists ${n} tracks`);

await page.fill("#search", "λαμψη");
await page.waitForFunction(() => document.querySelectorAll("#tracks li").length === 1);
check((await page.locator("#tracks .title").first().textContent()) === "Λάμψη", "Greek search without accents finds Λάμψη");
await page.fill("#search", "");
await page.waitForFunction(() => document.querySelectorAll("#tracks li").length > 1);

await page.locator("#tracks li button", { hasText: "Play" }).first().click();
await page.waitForSelector("#webamp #webamp, #webamp [id*='main-window'], #webamp canvas, #webamp .webamp-window", { timeout: 20000 }).catch(() => {});
const webampNodes = await page.locator("#webamp *").count();
check(webampNodes > 5, `Webamp rendered (${webampNodes} nodes)`);

const auth = { authorization: `Bearer ${token}`, "content-type": "application/json" };
let sess;
for (let i = 0; i < 40; i++) {
  sess = await (await fetch(`${base}/v1/session`, { headers: auth })).json();
  if (sess.track) break;
  await new Promise((r) => setTimeout(r, 250));
}
check(!!sess?.track && !!sess?.client_id, `session reports the playing track (${sess?.track?.title}, state ${sess?.state})`);

const cmd = await fetch(`${base}/v1/session`, { method: "POST", headers: auth, body: JSON.stringify({ action: "pause" }) });
check(cmd.status === 202, "a remote pause command is accepted while the player is connected");
await new Promise((r) => setTimeout(r, 1500));
const after = await (await fetch(`${base}/v1/session`, { headers: auth })).json();
check(after.state === "paused", `player obeyed the command (state ${after.state})`);

// ---- radio stations: add one through the API, find it in the UI, play it, control it ----
const addRes = await fetch(`${base}/v1/stations`, { method: "POST", headers: auth, body: JSON.stringify({ name: "Smoke Test FM", url: stationUrl, category: "Music" }) });
const byNameBefore = await addRes.clone().json();
check(addRes.status === 201, "a station can be added through the API");
await page.reload();
await page.waitForSelector("#app", { state: "visible" });
await page.fill("#st-search", "smoke test");
await page.waitForFunction(() => document.querySelectorAll("#st-list li").length === 1);
await page.locator("#st-list li button", { hasText: "Play" }).click();
await page.waitForFunction(() => { const a = document.getElementById("st-audio"); return !a.paused && a.currentTime > 0.3; }, null, { timeout: 15000 }).catch(() => {});
const playing = await page.evaluate(() => { const a = document.getElementById("st-audio"); return { paused: a.paused, t: a.currentTime, err: a.error?.code ?? null, net: a.networkState, ready: a.readyState, src: a.src, pauses: window.__pauses, status: document.getElementById("st-status").textContent }; });
check(!playing.paused && playing.t > 0, `the station plays in the browser (${JSON.stringify(playing)})`);
let ssess;
for (let i = 0; i < 40; i++) {
  ssess = await (await fetch(`${base}/v1/session`, { headers: auth })).json();
  if (ssess.station) break;
  await new Promise((r) => setTimeout(r, 250));
}
check(ssess?.station?.name === "Smoke Test FM" && ssess.state === "playing", `session reports the station (${ssess?.station?.name}, ${ssess?.state})`);
const pause2 = await fetch(`${base}/v1/session`, { method: "POST", headers: auth, body: JSON.stringify({ action: "pause" }) });
await new Promise((r) => setTimeout(r, 1500));
const stPaused = await page.evaluate(() => document.getElementById("st-audio").paused);
check(pause2.status === 202 && stPaused, "a remote pause stops the station");
// the widget's "play this station": a remote command that starts a station in the player
await page.evaluate(() => document.getElementById("st-audio").pause());
await new Promise((r) => setTimeout(r, 500));
const stId = byNameBefore?.id;
const playCmd = await fetch(`${base}/v1/session`, { method: "POST", headers: auth, body: JSON.stringify({ action: "play", station_id: stId }) });
await page.waitForFunction(() => { const a = document.getElementById("st-audio"); return !a.paused && a.currentTime > 0; }, null, { timeout: 15000 }).catch(() => {});
const afterPlay = await page.evaluate(() => { const a = document.getElementById("st-audio"); return { paused: a.paused, t: a.currentTime, pauses: window.__pauses.slice(-3), src: a.src, status: document.getElementById("st-status").textContent }; });
check(playCmd.status === 202 && !afterPlay.paused, `a remote play command with a station id starts that station (HTTP ${playCmd.status}, stId=${stId}, ${JSON.stringify(afterPlay)}, errors=${JSON.stringify(errors.slice(-3))})`);
const dead = await fetch(`${base}/v1/stations`, { method: "POST", headers: auth, body: JSON.stringify({ name: "Dead FM", url: "http://127.0.0.1:9/none" }) });
const job = await (await fetch(`${base}/v1/jobs`, { method: "POST", headers: auth, body: JSON.stringify({ kind: "check_stations" }) })).json();
let res;
for (let i = 0; i < 60; i++) {
  res = await (await fetch(`${base}/v1/jobs/${job.id}`, { headers: auth })).json();
  if (res.state === "done" || res.state === "failed") break;
  await new Promise((r) => setTimeout(r, 500));
}
const list = await (await fetch(`${base}/v1/stations?limit=50`, { headers: auth })).json();
const byName = Object.fromEntries(list.items.map((s) => [s.name, s]));
check(dead.status === 201 && byName["Smoke Test FM"]?.alive === true && byName["Dead FM"]?.alive === false, `the check marks the live station alive and the dead one dead (${byName["Dead FM"]?.error})`);

const relevant = errors.filter((e) => !/favicon|Failed to load resource.*(404|401)/.test(e));
check(relevant.length === 0, `no console/page errors${relevant.length ? ": " + relevant.join(" | ") : ""}`);
await browser.close();
radio.close();
process.exit(fails.length ? 1 : 0);
