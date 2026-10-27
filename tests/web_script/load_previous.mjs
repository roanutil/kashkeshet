// Checks the channel page's "Show previous" button in jsdom, against a
// running web preview. Not run by `cargo test`: it needs Node and jsdom.
//
//   cargo run --example web_preview -- --bind 127.0.0.1:7499 --seed 7
//   cd tests/web_script && npm ci && node load_previous.mjs http://127.0.0.1:7499
//
// jsdom has no layout, so this checks what the list holds, not that the
// reader's place stays put on screen; check that in a browser.
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";

const base = process.argv[2];
if (!base) throw new Error("usage: node load_previous.mjs <preview base URL>");

/// The first channel whose page has earlier messages to load.
async function longChannel() {
  const home = await (await fetch(`${base}/`)).text();
  const ids = new Set([...home.matchAll(/href="\/channels\/(ch_[a-z0-9]+)"/g)].map((m) => m[1]));
  for (const id of ids) {
    const html = await (await fetch(`${base}/channels/${id}`)).text();
    if (/id="older"(?! hidden)/.test(html)) return { id, html };
  }
  throw new Error("no channel with earlier messages");
}

/// The channel page in jsdom, with its script running. `hold()` makes the
/// next fetch wait until `release()`; `live(message)` delivers a live message.
function page({ id, html }) {
  let gate = null;
  const listeners = {};
  const dom = new JSDOM(html, {
    url: `${base}/channels/${id}`,
    runScripts: "dangerously",
    beforeParse(window) {
      window.TextEncoder = TextEncoder;
      window.fetch = async (url, opts) => {
        const res = await fetch(new URL(url, base), opts);
        if (gate) await gate.promise;
        return res;
      };
      window.EventSource = class {
        addEventListener(name, fn) { listeners[name] = fn; }
        close() {}
      };
      window.scrollTo = () => {};
      window.scrollBy = () => {};
    },
  });
  const { document } = dom.window;
  const link = document.querySelector("#older a");
  return {
    shown: () => [...document.querySelectorAll("ol.messages li.msg")].map((li) => Number(li.dataset.id)),
    older: document.getElementById("older"),
    link,
    click: () => link.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true, cancelable: true })),
    hold() { let release; gate = { promise: new Promise((r) => { release = r; }) }; gate.release = release; },
    release() { const g = gate; gate = null; g.release(); },
    live: (m) => listeners.message({ data: JSON.stringify(m) }),
  };
}

const settle = (ms) => new Promise((r) => setTimeout(r, ms));

// Pressing until the start loads every message once, oldest first, and
// keeps the first page at the end.
{
  const p = page(await longChannel());
  const first = p.shown();
  let presses = 0;
  while (!p.older.hidden && presses < 100) {
    const before = p.link.dataset.before;
    p.click();
    for (let i = 0; i < 200 && p.link.dataset.before === before && !p.older.hidden; i++) await settle(10);
    presses += 1;
  }
  const all = p.shown();
  assert.ok(p.older.hidden, "the button hides at the start");
  assert.ok(presses >= 2, "several pages");
  assert.deepEqual(all, [...all].sort((a, b) => a - b), "oldest first");
  assert.equal(new Set(all).size, all.length, "each once");
  assert.deepEqual(all.slice(-first.length), first, "the first page stays at the end");
  console.log(`ok: ${presses} presses, ${first.length} → ${all.length} messages`);
}

// A page that comes back after the live view dropped the message it was
// asked for is not added; the next press joins on exactly.
{
  const channel = await longChannel();
  const p = page(channel);
  const first = p.shown();
  p.hold();
  p.click();
  await settle(50);
  p.live({
    id: first.at(-1) + 1_000_000, channel_id: channel.id, channel_name: "x",
    agent: "amber-falcon", from_operator: false, body: "live",
    created_at: "2026-10-27T00:00:00Z", run_id: 1,
  });
  const trimmed = p.shown();
  assert.notEqual(trimmed[0], first[0], "the live message trimmed the oldest");
  assert.equal(p.link.dataset.before, String(trimmed[0]), "the button points before the new oldest");
  p.release();
  await settle(200);
  assert.deepEqual(p.shown(), trimmed, "the late page is dropped");
  p.click();
  for (let i = 0; i < 200 && p.shown().length === trimmed.length; i++) await settle(10);
  const expected = await (await fetch(`${base}/channels/${channel.id}/messages?before_id=${trimmed[0]}`)).json();
  const now = p.shown();
  assert.deepEqual(now.slice(0, now.length - trimmed.length), expected.messages.map((m) => m.id), "the next press joins on");
  console.log("ok: a late page after a trim is dropped");
}
