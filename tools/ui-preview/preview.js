// Renders the interface with a stubbed Tauri bridge and writes screenshots.
//
// The development machine has no display and no webview, so without this there
// is no way to look at the interface at all short of building the Windows
// binary and moving it to another computer.
//
//   node tools/ui-preview/preview.js [outputDir]
//
// Needs Playwright's Chromium. It never touches the Rust side: every command
// is answered by the stub below, so what it shows is the front end alone.

const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "../..");
const UI = path.join(ROOT, "ui");
const OUT = path.resolve(process.argv[2] ?? path.join(ROOT, "target", "ui-preview"));

function chromium() {
  for (const where of [
    "playwright",
    "/usr/lib/node_modules/playwright",
    path.join(ROOT, "node_modules/playwright"),
  ]) {
    try {
      return require(where).chromium;
    } catch {}
  }
  throw new Error("Playwright not found. Install it, or pass its path in NODE_PATH.");
}

/// One frame stands in for the whole folder: the real preview extracted from
/// the fixture, or a flat grey if it has not been produced yet.
function frame() {
  const extracted = path.join(OUT, "frame.jpg");
  if (fs.existsSync(extracted)) return `file://${extracted}`;
  return (
    "data:image/svg+xml;base64," +
    Buffer.from(
      `<svg xmlns="http://www.w3.org/2000/svg" width="6000" height="4000">
         <rect width="100%" height="100%" fill="#4a4a4a"/></svg>`,
    ).toString("base64")
  );
}

const stub = (frameUrl) => `
const photos = Array.from({ length: 1240 }, (_, i) => ({
  name: "IMG_" + (4820 + i) + ".CR3",
  width: 6000, height: 4000,
  rotation: i === 3 ? 90 : 0,
  mirrored: false,
}));
const marks = photos.map(() => ({ rating: 0, label: null }));
const assigned = photos.map(() => null);
const collections = [];
let settings = { xmpCompat: "lightroom" };
const at = (index) => ({ index, mark: marks[index], collection: assigned[index] });
const commands = {
  pick_folder: () => "D:\\\\fotos\\\\2026-09-05-interlagos",
  open_folder: () => ({
    folder: "D:\\\\fotos\\\\2026-09-05-interlagos",
    photos, marks, collections, assigned,
  }),
  get_settings: () => settings,
  set_settings: (a) => (settings = a.settings),
  set_index: () => {},
  set_star: ({ index, stars }) => {
    marks[index].rating = marks[index].rating === stars ? 0 : stars;
    return at(index);
  },
  toggle_reject: ({ index }) => {
    marks[index].rating = marks[index].rating === -1 ? 0 : -1;
    return at(index);
  },
  toggle_label: ({ index }) => {
    marks[index].label = marks[index].label ? null : "Green";
    return at(index);
  },
  new_collection: ({ name }) => {
    const clean = name.trim();
    if (!clean) throw new Error("o nome não pode ser vazio");
    if (collections.some((c) => c.toLowerCase() === clean.toLowerCase())) {
      throw new Error("já existe uma coleção chamada " + clean);
    }
    collections.push(clean);
    return collections;
  },
  assign: ({ index, collection }) => {
    assigned[index] = collection;
    return at(index);
  },
  undo: () => null,
  redo: () => null,
  plan: () => ({
    evaluated: 487, sidecars: 312, rejected: 103, untouched: 753,
    moves: [
      { collection: "porsche", photos: 84, files: 168 },
      { collection: "ferrari", photos: 61, files: 61 },
    ],
    blockers: [],
  }),
  apply: () => ({
    sidecars: 312, moved: 145, filesMoved: 229,
    rejected: 103, untouched: 753, error: null,
  }),
};
window.__TAURI__ = {
  core: {
    invoke: async (name, args = {}) => commands[name](args),
    convertFileSrc: () => ${JSON.stringify(frameUrl)},
  },
};
`;

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium().launch();
  const page = await browser.newPage({ viewport: { width: 1400, height: 880 } });

  const problems = [];
  page.on("pageerror", (e) => problems.push(String(e)));
  page.on("console", (m) => m.type() === "error" && problems.push(m.text()));

  await page.addInitScript(stub(frame()));
  await page.goto(`file://${path.join(UI, "index.html")}`);
  await page.waitForTimeout(400);

  const shot = async (name) => {
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(OUT, `${name}.png`) });
    console.log(`  ${name}.png`);
  };

  const press = async (key, times = 1) => {
    for (let i = 0; i < times; i++) {
      await page.keyboard.press(key);
      await page.waitForTimeout(25);
    }
  };

  console.log(`writing to ${OUT}`);
  await shot("idle");

  await page.click("#open");
  await page.waitForTimeout(500);

  await press("h", 24);
  await press("g");
  await press(" ");
  await shot("marked");

  await press("h");
  await press("Backspace");
  await press("k");
  await shot("rejected");

  await press("k", 22);
  await shot("portrait");

  // Phase 3: a collection, the picker, and the Apply preview.
  await press("n");
  await page.fill("#collection-name", "porsche");
  await shot("new-collection");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(200);
  await shot("move");
  await press("1");

  await press("n");
  await page.fill("#collection-name", "ferrari");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(200);
  await press("Escape");
  await press("h");
  await press("m");
  await shot("move-two");
  await press("2");
  await shot("assigned");

  await page.keyboard.press("Control+Enter");
  await shot("apply");
  await page.keyboard.press("Escape");

  await press("c");
  await shot("settings");
  await press("Escape");

  await press("?");
  await shot("keys");
  await press("Escape");

  await page.keyboard.press("Control+Enter");
  await shot("report");

  console.log(problems.length ? `\nproblems:\n  ${problems.join("\n  ")}` : "\nno console errors");
  await browser.close();
  process.exitCode = problems.length ? 1 : 0;
})();
