// Opt-in check against REAL radio streams in a real (headless) browser. Needs network; never run in CI.
//   MUSEQ_URL=http://127.0.0.1:8099 MUSEQ_TOKEN=... node e2e/net-stations.mjs "Omarchy Radio" "Alpha 98.9"
// The server must have the station lists imported. Prints one line per station.
import { chromium } from "playwright-core";

const base = process.env.MUSEQ_URL, token = process.env.MUSEQ_TOKEN;
if (!base || !token) throw new Error("set MUSEQ_URL and MUSEQ_TOKEN");
const names = process.argv.slice(2);
if (!names.length) throw new Error("give station names to try");

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? "/usr/bin/chromium", args: ["--autoplay-policy=no-user-gesture-required", "--no-sandbox"] });
const page = await (await browser.newContext()).newPage();
const errors = [];
page.on("console", (m) => { if (m.type() === "error" && !/401|favicon/.test(m.text())) errors.push(m.text().slice(0, 140)); });
await page.goto(base);
await page.fill("#token", token);
await page.click("#login-form button");
await page.waitForSelector("#app", { state: "visible" });
let failed = 0;
for (const name of names) {
  errors.length = 0;
  await page.fill("#st-search", name);
  await page.waitForTimeout(600);
  const row = page.locator("#st-list li", { hasText: name }).first();
  if (!(await row.count())) { console.log(`MISSING  ${name}`); failed++; continue; }
  await row.locator("button", { hasText: "Play" }).click();
  let state = null;
  for (let i = 0; i < 40; i++) {
    state = await page.evaluate(() => { const a = document.getElementById("st-audio"); return { paused: a.paused, t: a.currentTime, err: a.error?.code ?? null, src: new URL(a.src).protocol }; });
    if (!state.paused && state.t > 1) break;
    await page.waitForTimeout(500);
  }
  const ok = !state.paused && state.t > 1;
  if (!ok) failed++;
  console.log(`${ok ? "PLAYS   " : "FAILS   "} ${name}  (${state.src} stream, played ${state.t.toFixed(1)} s, media error ${state.err ?? "none"}${errors.length ? ", console: " + errors[0] : ""})`);
  await page.evaluate(() => document.getElementById("st-audio").pause());
}
await browser.close();
process.exit(failed ? 1 : 0);
