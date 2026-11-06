// Headless smoke test of the built client against a real server.
// Usage: MUSEQ_URL=http://127.0.0.1:8099 MUSEQ_TOKEN=... node e2e/smoke.mjs   (CHROMIUM=/usr/bin/chromium)
import { chromium } from "playwright-core";

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
check(after.state === "paused" || after.state === "stopped", `player obeyed the command (state ${after.state})`);

const relevant = errors.filter((e) => !/favicon|Failed to load resource.*(404|401)/.test(e));
check(relevant.length === 0, `no console/page errors${relevant.length ? ": " + relevant.join(" | ") : ""}`);
await browser.close();
process.exit(fails.length ? 1 : 0);
