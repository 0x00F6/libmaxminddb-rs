import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve, extname } from "node:path";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const output = fileURLToPath(new URL("../test-results/", import.meta.url));
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
};
const server = createServer(async (req, res) => {
  try {
    const pathname = decodeURIComponent(
      new URL(req.url, "http://localhost").pathname,
    );
    if (!pathname.startsWith("/libmaxminddb-rs/"))
      throw new Error("Wrong project base");
    const file = resolve(
      root,
      pathname.slice("/libmaxminddb-rs/".length) || "index.html",
    );
    if (!file.startsWith(root)) throw new Error("Outside build");
    const body = await readFile(file);
    res.writeHead(200, {
      "Content-Type": mime[extname(file)] || "application/octet-stream",
    });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end("Not found");
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const executablePath =
  process.env.CHROMIUM_PATH ||
  [
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].find(existsSync);
let browser;
try {
  assert.ok(
    executablePath,
    "Install Chromium or set CHROMIUM_PATH to its executable",
  );
  browser = await chromium.launch({
    executablePath,
    headless: true,
    args: [
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--disable-gpu",
      "--no-zygote",
    ],
  });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    reducedMotion: "reduce",
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("response", (r) => {
    if (r.status() >= 400) errors.push(`${r.status()} ${r.url()}`);
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/libmaxminddb-rs/`);
  await page.waitForSelector("#example-tabs button", { state: "attached" });
  assert.equal(await page.locator("html").getAttribute("data-theme"), "dark");
  assert.equal(await page.locator("#load-error").isVisible(), false);
  const loadedDocument = await page.evaluate(() => {
    window.spaSentinel = "same document";
    return performance.timeOrigin;
  });
  await mkdir(output, { recursive: true });
  await page.screenshot({
    path: resolve(output, "overview-dark.png"),
    fullPage: true,
  });
  await page.locator('nav [data-route="benchmarks"]').click();
  await page.waitForSelector("#benchmark-chart svg");
  assert.equal(await page.locator("#results-body tr").count(), 5);
  assert.match(
    await page.locator("#results-body").innerText(),
    /libmaxminddb \(C\)/,
  );
  await page.screenshot({
    path: resolve(output, "benchmarks-dark.png"),
    fullPage: true,
  });
  // Exercise every comparison category, workload and dimension from the public UI.
  for (const category of [
    "throughput",
    "latency",
    "concurrent",
    "memory",
    "writer",
  ]) {
    await page.locator(`[data-category="${category}"]`).click();
    const metrics = await page
      .locator("#metric-select option")
      .evaluateAll((els) => els.map((e) => e.value));
    for (const metric of metrics) {
      await page.locator("#metric-select").selectOption(metric);
      if (await page.locator(".segmented").isVisible()) {
        for (const family of ["ipv6", "ipv4"]) {
          await page.locator(`[data-family="${family}"]`).click();
          assert.ok((await page.locator("#results-body tr").count()) >= 4);
        }
      }
      const dimensions = await page
        .locator("#dimension-select option")
        .evaluateAll((els) => els.map((e) => e.value));
      for (const dimension of dimensions)
        await page.locator("#dimension-select").selectOption(dimension);
      const text = await page.locator("#results-body").innerText();
      assert.ok(!/NaN|undefined/.test(text));
      assert.ok(text.includes("libmaxminddb-rs"));
    }
  }
  assert.equal(await page.locator("#results-body tr").count(), 2);
  assert.match(await page.locator("#results-body").innerText(), /mmdbwriter/);
  await page.locator('nav [data-route="examples"]').click();
  for (const name of ["quickstart", "editor_merge", "concurrent_editor"]) {
    await page.locator(`[data-example="${name}"]`).click();
    assert.match(await page.locator("#example-code").innerText(), /fn main\(/);
    assert.ok((await page.locator("#example-code .hljs-keyword").count()) > 0);
  }
  assert.match(await page.locator("#example-code").innerText(), /lookup/);
  await page.screenshot({
    path: resolve(output, "examples-dark.png"),
    fullPage: true,
  });
  assert.equal(
    await page.evaluate(() => performance.timeOrigin),
    loadedDocument,
  );
  assert.equal(await page.evaluate(() => window.spaSentinel), "same document");
  await page.locator("#theme-toggle").click();
  await page.reload();
  await page.waitForSelector("#example-tabs button", { state: "attached" });
  assert.equal(await page.locator("html").getAttribute("data-theme"), "light");
  await page.locator('nav [data-route="benchmarks"]').click();
  await page.locator('[data-category="latency"]').click();
  await page.locator('[data-family="ipv6"]').click();
  await page.screenshot({
    path: resolve(output, "benchmarks-light.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  for (const route of ["overview", "benchmarks", "examples", "methodology"]) {
    await page.locator(`nav [data-route="${route}"]`).click();
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
      `${route} overflows on mobile`,
    );
  }
  await page.locator('nav [data-route="benchmarks"]').click();
  await page.screenshot({
    path: resolve(output, "benchmarks-mobile.png"),
    fullPage: true,
  });
  await page.goBack();
  assert.equal(await page.locator("#methodology").isVisible(), true);
  assert.deepEqual(errors, []);
  console.log(
    "Browser checks passed: SPA routes, all benchmark controls, typed examples, highlighting, persistent themes, mobile layout, no console/network errors.",
  );
} finally {
  await browser?.close();
  await new Promise((r) => server.close(r));
}
