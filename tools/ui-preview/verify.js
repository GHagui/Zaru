// Browser regressions use only the public DOM and a controlled command bridge.
// The bridge clones replies like IPC; sharing objects would hide state bugs.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

exports.verify = async function verify({ browser, stub, root, out }) {
  const errors = [];
  const checked = [];
  const watch = page => {
    page.on("pageerror", error => errors.push(String(error)));
    page.on("console", message => { if (message.type() === "error") errors.push(message.text()); });
  };
  const fresh = async (viewport = { width: 1400, height: 900 }) => {
    const page = await browser.newPage({ viewport });
    watch(page);
    await page.addInitScript(stub);
    await page.goto(`file://${path.join(root, "ui/index.html")}`);
    await page.waitForFunction(() => document.querySelectorAll(".keyrow").length > 20);
    await page.evaluate(() => document.fonts.ready);
    return page;
  };
  const open = async page => {
    await page.click("#open");
    await page.waitForFunction(() => !document.body.classList.contains("idle") && document.body.getAttribute("aria-busy") === "false");
  };
  const shot = async (page, name) => {
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: path.join(out, `${name}.png`) });
  };
  const calls = (page, name) => page.evaluate(name => window.__preview.calls.filter(call => call.name === name), name);
  const check = (condition, message) => { assert.ok(condition, message); checked.push(message); };

  const page = await fresh();
  await open(page);
  await page.click('[data-command="star3"]');
  check(await page.locator('[data-command="star3"]').getAttribute("aria-pressed") === "true", "Mouse assigns three stars");
  check((await page.locator("#counter").innerText()).startsWith("1 /"), "Rating never advances");
  await page.keyboard.press("s");
  check(await page.locator("#rating .on").count() === 0, "Keyboard clears the same rating");
  await page.click("#label");
  await page.locator("#stage").focus();
  await page.keyboard.press("Space");
  check(await page.locator("#label").getAttribute("aria-pressed") === "false", "Mouse and keyboard share green-label behavior");
  await page.click("#reject");
  check((await page.locator("#counter").innerText()).startsWith("2 /"), "Reject advances immediately");
  await page.click('[data-command="prev"]');
  check(await page.locator("#reject").getAttribute("aria-pressed") === "true", "Rejected state survives navigation");
  await page.click('[data-command="star5"]');
  check(await page.locator("#reject").getAttribute("aria-pressed") === "false", "Rating replaces rejection");
  await page.locator('[data-command="star5"]').focus();
  await page.keyboard.press("Control+Enter");
  await page.waitForSelector("#apply:not([hidden])");
  check(await page.locator("#apply").isVisible(), "Ctrl+Enter opens review even with a button focused");
  await page.keyboard.press("Escape");

  await page.click('nav [data-command="filter"]');
  await page.locator("#filter-list button").nth(6).click();
  check(await page.locator("#no-results").isVisible(), "An empty filter remains selected");
  check(await page.locator("#counter").innerText() === "0 / 0", "Empty filters show zero photos");
  const marksBefore = (await calls(page, "set_star")).length;
  await page.keyboard.press("g");
  check((await calls(page, "set_star")).length === marksBefore, "Empty filters cannot mutate photo zero");
  check(await page.locator('#ring .shown').count() === 0, "An empty filter hides every image");
  await page.click('[data-command="clearFilter"]');
  check(!await page.locator("#no-results").isVisible(), "Clear filter resumes the session");

  await page.click('[data-command="compare"]');
  check(await page.locator("#ring .shown").count() === 2, "Comparison shows both panes even for the same photo");
  await page.click("#zoom");
  const transformBefore = await page.locator("#ring .shown img").first().evaluate(img => img.style.transform.match(/scale\(([^)]+)/)[1]);
  await page.click('[data-command="next"]');
  const transforms = await page.locator("#ring .shown img").evaluateAll(images => images.map(img => img.style.transform.match(/scale\(([^)]+)/)[1]));
  check(transforms.every(scale => scale === transformBefore), "Zoom stays synchronized across comparison and navigation");
  const boxes = await page.locator("#ring .shown").evaluateAll(nodes => nodes.map(node => ({ x: node.getBoundingClientRect().x, width: node.clientWidth, overflow: getComputedStyle(node).overflow })));
  check(boxes.every(box => box.overflow === "hidden") && Math.abs(boxes[0].x - boxes[1].x) > 600, "Comparison clips images into separate panes");
  await page.click('[data-command="compare"]');
  await page.keyboard.press("Escape");

  await page.keyboard.press("m");
  await page.keyboard.press("n");
  check(await page.locator("#newCollection").isVisible(), "New-collection shortcut works inside the empty picker");
  await page.fill("#collection-name", "porsche");
  await page.keyboard.press("Enter");
  await page.waitForSelector("#move:not([hidden])");
  await page.locator("#move-list button").first().click();
  check(await page.locator("#collection").innerText() === "porsche", "Collection picker supports clicking");
  await page.click('nav [data-command="collections"]');
  check((await page.locator("#stage").boundingBox()).x === 260, "Desktop collections reserve space beside the photo");
  await page.locator("#sidebar-list button").first().click({ modifiers: ["Alt"] });
  check((await calls(page, "assign_burst")).length === 1, "Alt-click assigns or clears the whole burst");
  await page.locator("#sidebar-list button").first().click();
  check(await page.locator("#sidebar-list small").innerText() === "1", "Collection counts refresh after changes");
  await page.click('#collections-sidebar [data-command="collections"]');
  check((await page.locator("#stage").boundingBox()).x === 0, "Closing collections restores the photo area");

  await page.click("#config");
  await page.locator('#settings [data-close]').last().focus();
  await page.keyboard.press("Tab");
  check(await page.evaluate(() => document.querySelector("#settings").contains(document.activeElement)), "Dialog traps forward Tab");
  await page.keyboard.press("Shift+Tab");
  check(await page.evaluate(() => document.querySelector("#settings").contains(document.activeElement)), "Dialog traps reverse Tab");
  await page.locator('.keyrow[data-action="zoom"]').click();
  await page.keyboard.press("x");
  check((await page.locator("#zoom").getAttribute("title")).endsWith("· x"), "Shortcut hints follow remapping");
  await page.keyboard.press("Escape");
  check(await page.evaluate(() => document.activeElement.id === "config"), "Dialog restores the invoking control's focus");
  await page.keyboard.press("x");
  check((await page.locator("#zoom").innerText()).startsWith("100%"), "Remapped zoom works");
  await page.keyboard.press("Escape");

  // A delayed command lets repeated Enter/click attempts race the same request.
  await page.evaluate(() => {
    const original = window.__preview.commands.apply;
    window.__preview.commands.apply = async () => {
      await new Promise(resolve => setTimeout(resolve, 250));
      return original();
    };
  });
  await page.click('nav [data-command="apply"]');
  await page.locator('#apply .dialog-actions [data-close]').focus();
  await page.keyboard.press("Enter");
  check((await calls(page, "apply")).length === 0, "Enter on Cancel never applies files");
  await page.click('nav [data-command="apply"]');
  await page.click("#apply-go");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Control+Enter");
  await page.waitForSelector("#report:not([hidden])");
  check((await calls(page, "apply")).length === 1, "Applying blocks duplicate submissions");
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    const original = window.__preview.commands.plan;
    window.__preview.commands.plan = () => ({ ...original(), blockers: ["IMG_4820.CR3 já existe no destino"] });
  });
  await page.click('nav [data-command="apply"]');
  check(await page.locator("#apply-go").isDisabled(), "Collisions disable Apply");
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    const plan = window.__preview.commands.plan();
    window.__preview.commands.plan = () => ({ ...plan, blockers: [] });
    window.__preview.commands.apply = () => { throw new Error("Não foi possível gravar: disco cheio"); };
  });
  await page.click('nav [data-command="apply"]');
  await page.click("#apply-go");
  await page.waitForFunction(() => document.body.getAttribute("aria-busy") === "false");
  check(await page.locator("#apply").isVisible() && await page.locator("#notice").isVisible(), "Application errors keep the review and show feedback");
  check(await page.locator("#apply-go").isEnabled(), "A failed request re-enables retry");
  await page.keyboard.press("Escape");
  await page.close();

  const recovery = await fresh();
  await recovery.evaluate(() => {
    window.__preview.commands.recovery_offer = () => ({ marked: 3, assigned: 0, collections: 0 });
    window.__preview.commands.restore_session = () => {
      window.__preview.marks[0].rating = 4;
      return window.__preview.commands.open_folder();
    };
  });
  await open(recovery);
  await recovery.waitForSelector("#recovery:not([hidden])");
  check(await recovery.evaluate(() => document.querySelector("#recovery").contains(document.activeElement)), "Recovery receives focus after loading");
  await recovery.keyboard.press("Escape");
  check((await calls(recovery, "discard_recovery")).length === 0, "Escape preserves the recovery draft");
  await recovery.click('nav [data-command="open"]');
  await recovery.waitForSelector("#recovery:not([hidden])");
  await recovery.click("#recovery-go");
  await recovery.waitForSelector("#recovery", { state: "hidden" });
  check(await recovery.locator('[data-command="star4"]').getAttribute("aria-pressed") === "true", "Restoration adopts recovered marks");
  check((await calls(recovery, "discard_recovery")).length === 0, "Restoring never discards before loading");
  await recovery.click('nav [data-command="open"]');
  await recovery.waitForSelector("#recovery:not([hidden])");
  await recovery.click("#recovery-discard");
  await recovery.waitForSelector("#recovery", { state: "hidden" });
  check((await calls(recovery, "discard_recovery")).length === 1, "Explicit discard removes the draft once");
  await recovery.evaluate(() => { window.__preview.commands.restore_session = () => null; });
  await recovery.click('nav [data-command="open"]');
  await recovery.waitForSelector("#recovery:not([hidden])");
  await recovery.click("#recovery-go");
  await recovery.waitForFunction(() => document.body.getAttribute("aria-busy") === "false");
  check(await recovery.locator("#recovery").isVisible() && await recovery.locator("#recovery-go").isEnabled(), "Unavailable recovery leaves an actionable dialog");
  check((await calls(recovery, "discard_recovery")).length === 1, "Failed restoration does not discard the draft");
  await recovery.close();

  // Save matched before/after screenshots using tracked baseline sources.
  const baseline = {};
  for (const file of ["index.html", "app.css", "app.js"]) {
    baseline[file] = execFileSync("git", ["-c", `safe.directory=${root}`, "show", `HEAD:ui/${file}`], { cwd: root });
  }
  const framePath = path.join(out, "frame.jpg");
  const baselineStub = fs.existsSync(framePath) ? stub.replace(`file://${framePath}`, `data:image/jpeg;base64,${fs.readFileSync(framePath).toString("base64")}`) : stub;
  for (const viewport of [{ width: 1400, height: 900 }, { width: 1024, height: 768 }, { width: 800, height: 600 }]) {
    const size = `${viewport.width}x${viewport.height}`;
    const before = await browser.newPage({ viewport });
    await before.route("http://zaru-preview.test/**", route => {
      const name = new URL(route.request().url()).pathname.slice(1) || "index.html";
      const body = baseline[name] ?? (name.startsWith("fonts/") ? fs.readFileSync(path.join(root, "ui", name)) : "");
      return route.fulfill({ body, contentType: name.endsWith(".css") ? "text/css" : name.endsWith(".js") ? "text/javascript" : name.endsWith(".woff2") ? "font/woff2" : "text/html" });
    });
    await before.addInitScript(baselineStub);
    await before.goto("http://zaru-preview.test/index.html");
    await shot(before, `before-${size}-idle`);
    await before.click("#open");
    await before.waitForSelector("body:not(.idle)");
    await before.keyboard.press("g");
    await shot(before, `before-${size}-marked`);
    await before.keyboard.press("c");
    await shot(before, `before-${size}-settings`);
    await before.close();

    const screen = await fresh(viewport);
    await shot(screen, `after-${size}-idle`);
    await open(screen);
    await screen.click('[data-command="star5"]');
    await shot(screen, `after-${size}-marked`);
    const overflow = await screen.evaluate(() => [...document.querySelectorAll(".bar button, .bar .counter")].filter(node => node.getClientRects().length).filter(node => {
      const b = node.getBoundingClientRect();
      return b.x < 0 || b.right > innerWidth + 1 || b.bottom > innerHeight + 1 || node.scrollWidth > node.clientWidth + 2;
    }).map(node => node.textContent));
    check(overflow.length === 0, `${size}: controls fit without clipping (${overflow.join(", ")})`);
    await screen.keyboard.press("h");
    await screen.keyboard.press("h");
    await screen.keyboard.press("h");
    await shot(screen, `after-${size}-portrait`);
    await screen.click('[data-command="compare"]');
    await shot(screen, `after-${size}-compare`);
    await screen.click('[data-command="compare"]');
    await screen.click('nav [data-command="collections"]');
    await screen.click('#collections-sidebar [data-command="newCollection"]');
    await screen.fill("#collection-name", "Seleção para entrega");
    await screen.click("#collection-create");
    await screen.locator("#move-list button").first().click();
    await shot(screen, `after-${size}-collections`);
    await screen.click('#collections-sidebar [data-command="collections"]');
    await screen.click('nav [data-command="filter"]');
    await screen.locator("#filter-list button").nth(5).click();
    await shot(screen, `after-${size}-empty-filter`);
    await screen.click('[data-command="clearFilter"]');
    await screen.keyboard.press("c");
    await shot(screen, `after-${size}-settings`);
    await screen.locator('.keyrow[data-action="reject"]').scrollIntoViewIfNeeded();
    const keyClipped = await screen.locator('.keyrow[data-action="reject"] kbd').evaluate(node => node.scrollWidth > node.clientWidth);
    check(!keyClipped, `${size}: Backspace label fits its control`);
    await screen.keyboard.press("Escape");
    await screen.click('nav [data-command="apply"]');
    await shot(screen, `after-${size}-apply`);
    await screen.click("#apply-go");
    await screen.waitForSelector("#report:not([hidden])");
    await shot(screen, `after-${size}-report`);
    await screen.keyboard.press("Escape");
    await screen.evaluate(() => { window.__preview.commands.open_folder = () => { throw new Error("Nenhum arquivo CR3 nesta pasta."); }; });
    await screen.click('nav [data-command="open"]');
    await screen.waitForFunction(() => document.body.getAttribute("aria-busy") === "false");
    check(await screen.locator("#notice").isVisible(), `${size}: folder errors remain visible`);
    await shot(screen, `after-${size}-error`);
    await screen.close();
  }
  assert.deepEqual(errors, [], "No browser errors");
  console.log(`\n${checked.length} browser assertions passed; responsive before/after screenshots saved.`);
};
