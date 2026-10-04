import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve, extname } from "node:path";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("../dist/", import.meta.url));
const output = fileURLToPath(new URL("../test-results/", import.meta.url));
const datasets = JSON.parse(
  await readFile(resolve(root, "data/benchmark-datasets.json"), "utf8"),
);
const measurements = new Map(
  await Promise.all(
    datasets.map(async (d) => [
      d.id,
      JSON.parse(await readFile(resolve(root, "data", d.file), "utf8")),
    ]),
  ),
);
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
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
  await page.locator(".project-logo").evaluate((img) => img.decode());
  assert.ok(
    await page.locator(".project-logo").evaluate((img) => img.naturalWidth > 0),
  );
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
  await page.locator('[data-category="latency"]').click();
  assert.equal(await page.locator("#percentile-legend").isVisible(), true);
  for (const [family, firstLibrary, p50, p99] of [
    ["ipv4", "libmaxminddb-rs", 30, 441],
    ["ipv6", "libmaxminddb (C)", 30, 70],
  ]) {
    await page.locator(`[data-family="${family}"]`).click();
    assert.match(
      await page.locator("#chart-title").innerText(),
      /Candlestick Percentile Rank/,
    );
    assert.match(
      await page.locator("#chart-source").getAttribute("href"),
      new RegExp(`candlestick-percentiles-${family}\\.svg$`),
    );
    assert.equal(await page.locator("#results-head th").count(), 7);
    const firstRow = await page
      .locator("#results-body tr")
      .first()
      .locator("td")
      .allTextContents();
    assert.match(
      firstRow[0],
      new RegExp(firstLibrary.replace(/[()]/g, "\\$&")),
    );
    assert.equal(firstRow[3], `${p50} ns`);
    assert.equal(firstRow[5], `${p99} ns`);
    const svgText = await page.locator("#benchmark-chart svg").textContent();
    assert.equal((svgText.match(/p50 /g) || []).length, 5);
    assert.equal((svgText.match(/p99 /g) || []).length, 5);
    assert.ok(svgText.includes(`p50 ${p50}`));
    assert.ok(svgText.includes(`p99 ${p99}`));
  }
  await page.locator('[data-family="ipv4"]').click();
  await page
    .locator("#benchmark-chart svg")
    .getByText("p99 441", { exact: true })
    .hover();
  await page.waitForFunction(() =>
    [...document.querySelectorAll("#benchmark-chart div")].some((el) =>
      el.textContent.includes("max: 17,230 ns"),
    ),
  );
  await page.mouse.move(0, 0);
  await page.screenshot({
    path: resolve(output, "latency-dark.png"),
    fullPage: true,
  });
  await page.locator("#metric-select").selectOption("scaling");
  assert.equal(await page.locator("#percentile-legend").isVisible(), false);
  assert.equal(await page.locator("#results-head th").count(), 3);
  // Switching architecture must change data, sources and methodology together.
  for (const dataset of datasets) {
    await page.locator("#architecture-select").selectOption(dataset.id);
    assert.equal(new URL(page.url()).searchParams.get("arch"), dataset.id);
    const data = measurements.get(dataset.id);
    assert.ok(
      (await page.locator("#snapshot-context").innerText()).includes(
        dataset.id,
      ),
    );
    assert.equal(
      await page.locator("#machine-label").innerText(),
      data.context.cpu,
    );
    assert.ok(
      (await page.locator("#export-data").getAttribute("href")).endsWith(
        dataset.file,
      ),
    );
    assert.ok(
      (await page.locator("#raw-link").getAttribute("href")).endsWith(
        data.context.rawResults,
      ),
    );
    assert.ok(
      (await page.locator("#report-link").getAttribute("href")).endsWith(
        data.context.report,
      ),
    );
    assert.equal(
      await page.locator("#run-link").getAttribute("href"),
      data.context.workflowRunUrl,
    );
    await page.locator('[data-category="throughput"]').click();
    await page.locator('[data-family="ipv4"]').click();
    const expectedRate = data.charts
      .find((c) => c.id === "throughput-ipv4-random")
      .points.find((p) => p.library === "libmaxminddb-rs").value;
    const rateText = await page
      .locator("#results-body tr")
      .filter({ hasText: "libmaxminddb-rs" })
      .locator("td")
      .last()
      .innerText();
    assert.equal(Number(rateText.replace(/[^0-9.]/g, "")), expectedRate);
    await page.locator('[data-category="latency"]').click();
    for (const family of ["ipv4", "ipv6"]) {
      await page.locator(`[data-family="${family}"]`).click();
      const source = data.charts.find(
        (c) => c.id === `candlestick-percentiles-${family}`,
      );
      assert.equal(
        await page.locator("#chart-source").getAttribute("href"),
        source.source,
      );
      for (const point of source.points) {
        const cells = await page
          .locator("#results-body tr")
          .filter({ hasText: point.library })
          .locator("td")
          .allTextContents();
        assert.equal(
          Number(cells[3].replace(/[^0-9.]/g, "")),
          point.quantiles.p50,
        );
        assert.equal(
          Number(cells[5].replace(/[^0-9.]/g, "")),
          point.quantiles.p99,
        );
      }
    }
    await page.screenshot({
      path: resolve(output, `latency-${dataset.id}-dark.png`),
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
  }
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
  assert.equal(
    await page.locator("#architecture-select").inputValue(),
    datasets.at(-1).id,
  );
  assert.ok(
    (await page.locator("#environment-details").innerText()).includes(
      measurements.get(datasets.at(-1).id).context.cpu,
    ),
  );
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
  const labels = page
    .locator("#benchmark-chart svg text")
    .filter({ hasText: /^p(?:50|99) / });
  assert.equal(await labels.count(), 10);
  for (const label of await labels.all()) {
    const bounds = await label.boundingBox();
    assert.ok(
      bounds.x >= 0 && bounds.x + bounds.width <= 390,
      "Percentile label is clipped on mobile",
    );
  }
  await page.screenshot({
    path: resolve(output, "benchmarks-mobile.png"),
    fullPage: true,
  });
  await page.goBack();
  assert.equal(await page.locator("#methodology").isVisible(), true);
  // An explicit shared link takes precedence over the remembered architecture.
  await page.goto(
    `http://127.0.0.1:${server.address().port}/libmaxminddb-rs/?arch=${datasets[0].id}#benchmarks`,
  );
  await page.waitForSelector("#example-tabs button", { state: "attached" });
  assert.equal(
    await page.locator("#architecture-select").inputValue(),
    datasets[0].id,
  );
  assert.deepEqual(errors, []);
  console.log(
    "Browser checks passed: SPA routes, all benchmark controls, typed examples, highlighting, persistent themes, mobile layout, no console/network errors.",
  );
} finally {
  await browser?.close();
  await new Promise((r) => server.close(r));
}
