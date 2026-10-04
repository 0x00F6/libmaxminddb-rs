import "./style.css";
import * as echarts from "echarts/core";
import { BarChart, CustomChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  AriaComponent,
} from "echarts/components";
import { SVGRenderer } from "echarts/renderers";
import hljs from "highlight.js/lib/core";
import rust from "highlight.js/lib/languages/rust";
echarts.use([
  BarChart,
  CustomChart,
  GridComponent,
  TooltipComponent,
  AriaComponent,
  SVGRenderer,
]);
hljs.registerLanguage("rust", rust);

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const colors = [
  "#55dca7",
  "#e9ba70",
  "#80a8ef",
  "#cba2ed",
  "#6bc8d8",
  "#ef8c87",
];
const state = {
  category: "throughput",
  family: "ipv4",
  metric: "random",
  dimension: "",
  example: "quickstart",
};
const menus = {
  throughput: [
    ["random", "Random lookups"],
    ["hot", "Hot lookups"],
    ["sequential", "Sequential lookups"],
    ["absent", "Absent keys"],
  ],
  latency: [
    ["lookup", "Candlestick percentile rank"],
    ["scaling", "p99 by database size"],
  ],
  concurrent: [["workers", "Worker throughput"]],
  memory: [
    ["peak", "Peak RSS during lookups"],
    ["after-open", "RSS after open"],
  ],
  writer: [
    ["throughput", "Insert throughput"],
    ["build-time", "Total build time"],
    ["peak-rss", "Peak RSS"],
    ["p99", "Insertion p99"],
  ],
};
let benchmarkData, examples, chart, toastTimer;
let datasets = [];
const benchmarkSets = new Map();
const reducedMotion = window.matchMedia(
  "(prefers-reduced-motion: reduce)",
).matches;
function theme() {
  return document.documentElement.dataset.theme;
}
function updateThemeButton() {
  const dark = theme() === "dark";
  $("#theme-toggle").textContent = dark ? "☀" : "☾";
  $("#theme-toggle").setAttribute(
    "aria-label",
    `Switch to ${dark ? "light" : "dark"} theme`,
  );
}
$(".skip").addEventListener("click", (e) => {
  e.preventDefault();
  $("#content").focus();
});
$("#theme-toggle").addEventListener("click", () => {
  document.documentElement.dataset.theme =
    theme() === "dark" ? "light" : "dark";
  try {
    localStorage.setItem("mmdb-theme", theme());
  } catch {}
  updateThemeButton();
  renderChart();
});
updateThemeButton();
function route() {
  const requested = new URLSearchParams(location.search).get("arch");
  if (
    benchmarkSets.has(requested) &&
    benchmarkData !== benchmarkSets.get(requested)
  ) {
    selectArchitecture(requested);
  }
  const id = location.hash.slice(1);
  const active = ["benchmarks", "examples", "methodology"].includes(id)
    ? id
    : "overview";
  $$(".view").forEach((el) => {
    el.hidden = el.id !== active;
  });
  $$("[data-route]").forEach((el) => {
    el.classList.toggle("active", el.dataset.route === active);
    if (el.dataset.route === active) el.setAttribute("aria-current", "page");
    else el.removeAttribute("aria-current");
  });
  $("#breadcrumb").textContent = active;
  document.title = `${active[0].toUpperCase() + active.slice(1)} / libmaxminddb-rs`;
  if (active === "benchmarks")
    requestAnimationFrame(() => {
      renderChart();
      chart?.resize();
    });
  window.scrollTo({ top: 0, behavior: "instant" });
}
window.addEventListener("hashchange", route);
window.addEventListener("popstate", route);
route();
function toast(message) {
  clearTimeout(toastTimer);
  $("#toast").textContent = message;
  $("#toast").classList.add("show");
  toastTimer = setTimeout(() => $("#toast").classList.remove("show"), 2200);
}
async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copied to clipboard");
  } catch {
    toast("Copy unavailable. Select the code and copy it manually.");
  }
}
$$("[data-copy]").forEach((el) =>
  el.addEventListener("click", () => copy(el.dataset.copy)),
);
$("#copy-example").addEventListener("click", () =>
  copy(examples?.find((e) => e.id === state.example)?.code || ""),
);
function option(value, label) {
  const el = document.createElement("option");
  el.value = value;
  el.textContent = label;
  return el;
}
function selectChart() {
  const { category: c, family: f, metric: m } = state;
  const id =
    c === "throughput"
      ? `throughput-${f}-${m}`
      : c === "latency"
        ? m === "lookup"
          ? `candlestick-percentiles-${f}`
          : "database-size-scaling"
        : c === "concurrent"
          ? `concurrent-throughput${f === "ipv6" ? "-ipv6" : ""}`
          : c === "memory"
            ? `memory-rss-${m}`
            : `writer-${m}`;
  return benchmarkData?.charts.find((c) => c.id === id);
}
function controls(resetDimension = true) {
  $("#metric-select").replaceChildren(
    ...menus[state.category].map(([v, l]) => option(v, l)),
  );
  $("#metric-select").value = state.metric;
  $$("[data-category]").forEach((el) => {
    const on = el.dataset.category === state.category;
    el.classList.toggle("active", on);
    el.setAttribute("aria-pressed", String(on));
  });
  $$("[data-family]").forEach((el) => {
    const on = el.dataset.family === state.family;
    el.classList.toggle("active", on);
    el.setAttribute("aria-pressed", String(on));
  });
  $(".segmented").hidden =
    ["memory", "writer"].includes(state.category) ||
    (state.category === "latency" && state.metric === "scaling");
  const dims = [
    ...new Set(
      (selectChart()?.points || []).map((p) => p.dimension).filter(Boolean),
    ),
  ];
  $("#dimension-label").hidden = dims.length === 0;
  $("#dimension-select").replaceChildren(
    ...dims.map((d) =>
      option(d, d.endsWith("T") ? `${d.slice(0, -1)} workers` : `${d} entries`),
    ),
  );
  if (resetDimension || !dims.includes(state.dimension))
    state.dimension = dims.at(-1) || "";
  $("#dimension-select").value = state.dimension;
  renderChart();
}
$$("[data-category]").forEach((el) =>
  el.addEventListener("click", () => {
    state.category = el.dataset.category;
    state.metric = menus[state.category][0][0];
    controls();
  }),
);
$$("[data-family]").forEach((el) =>
  el.addEventListener("click", () => {
    state.family = el.dataset.family;
    controls(false);
  }),
);
$("#metric-select").addEventListener("change", (e) => {
  state.metric = e.target.value;
  controls();
});
$("#dimension-select").addEventListener("change", (e) => {
  state.dimension = e.target.value;
  renderChart();
});
function formatValue(value, unit) {
  return `${new Intl.NumberFormat("en", { maximumFractionDigits: unit === "MiB" ? 8 : 3 }).format(value)} ${unit}`;
}
function color(library) {
  const i = benchmarkData.context.libraries.findIndex(
    (l) => l.name === library,
  );
  return colors[i < 0 ? 0 : i % colors.length];
}
function textCell(text) {
  const el = document.createElement("td");
  el.textContent = text;
  return el;
}
function percentileSeries(points, text, surface, mobile) {
  return {
    type: "custom",
    name: "Lookup percentiles",
    dimensions: ["rank", "min", "p50", "p95", "p99"],
    encode: { x: [1, 2, 3, 4], y: 0, tooltip: [1, 2, 3, 4] },
    data: points.map((p, i) => [
      i,
      p.quantiles.min,
      p.quantiles.p50,
      p.quantiles.p95,
      p.quantiles.p99,
    ]),
    renderItem(params, api) {
      const p = points[params.dataIndex];
      const rank = api.value(0);
      const [min, p50, p95, p99] = [1, 2, 3, 4].map((d) =>
        api.coord([api.value(d), rank]),
      );
      const y = p50[1];
      const stroke = color(p.library);
      const segment = (x1, y1, x2, y2, ink = stroke) => ({
        type: "line",
        shape: { x1, y1, x2, y2 },
        style: { stroke: ink, lineWidth: 2 },
      });
      return {
        type: "group",
        children: [
          // Quantile geometry matches the source SVG, not financial OHLC data.
          segment(min[0], y, p99[0], y),
          segment(min[0], y - 7, min[0], y + 7),
          {
            type: "rect",
            shape: {
              x: p50[0],
              y: y - 12,
              width: Math.max(1, p95[0] - p50[0]),
              height: 24,
              r: 3,
            },
            style: { fill: stroke, fillOpacity: 0.35, stroke, lineWidth: 1.5 },
          },
          segment(p50[0], y - 12, p50[0], y + 12, text),
          {
            type: "circle",
            shape: { cx: p99[0], cy: y, r: 4 },
            style: { fill: surface, stroke, lineWidth: 2 },
          },
          ...[
            ["p50", p50[0], y - 24, "left"],
            ["p99", p99[0] + 9, y, "left"],
          ].map(([key, x, labelY, align]) => ({
            type: "text",
            style: {
              x,
              y: labelY,
              text: `${key} ${p.quantiles[key]}`,
              fill: text,
              font: `${mobile ? 9 : 11}px monospace`,
              align,
              verticalAlign: "middle",
            },
          })),
        ],
      };
    },
  };
}
function renderChart() {
  if (!benchmarkData) return;
  const data = selectChart();
  if (!data) return;
  const percentiles = data.id.startsWith("candlestick-");
  const high =
    state.category === "throughput" ||
    state.category === "concurrent" ||
    (state.category === "writer" && state.metric === "throughput");
  const points = data.points
    .filter((p) => !p.dimension || p.dimension === state.dimension)
    .sort((a, b) => (high ? b.value - a.value : a.value - b.value));
  $("#chart-title").textContent =
    state.category === "concurrent"
      ? `${state.family.toUpperCase()} concurrent throughput`
      : data.title;
  $("#chart-subtitle").textContent = state.dimension
    ? `${state.dimension.endsWith("T") ? "Workers" : "Database size"}: ${state.dimension} · same exported scenario`
    : data.subtitle;
  $("#direction").textContent = high
    ? "↑ HIGHER IS BETTER"
    : "↓ LOWER IS BETTER";
  $("#chart-source").href = data.source;
  $("#chart-context").textContent =
    `${points.length} implementations · ${benchmarkData.context.cpu}`;
  const notes = {
    throughput:
      "Throughput and p99 come from distinct benchmark measurements. Values here are the exported single-thread throughput; do not derive a latency by inverting them.",
    latency:
      "Tail latency (p99): 99% of the measured lookup samples completed at or below this value. Lower is better. Database-size scaling is a separate scenario.",
    concurrent: `4, 8 and 16-worker measurements on ${benchmarkData.context.availableCpus} available vCPUs. Worker counts above that oversubscribe the runner. The 1-worker export mixes fallback scenarios and is excluded; this chart does not claim linear speedup.`,
    memory:
      "Whole-process resident memory in mmap mode, including the prepared tree and runtime. Three isolated processes per size in the comparison protocol. Go reader RSS is not part of this export.",
    writer:
      "Same database sizes for the Rust writer and Go mmdbwriter. Throughput, total build duration and peak resident memory describe different costs.",
  };
  $("#metric-note").textContent = percentiles
    ? "Random lookup samples from the same run: p50 is the median, p95 and p99 are the 95th and 99th percentiles. The body spans p50–p95; the wick starts at the minimum and ends at the p99 marker. Ranked by p99, lowest first. Max is listed in the table and tooltip, outside the plotted range. These are measured quantiles, not confidence intervals."
    : notes[state.category];
  $("#percentile-legend").hidden = !percentiles;
  $(".results-grid").classList.toggle("percentile-results", percentiles);
  const metrics = percentiles ? ["min", "p50", "p95", "p99", "max"] : ["value"];
  const headers = [
    percentiles ? "Rank / Library" : "Library",
    "Measured version",
    ...metrics.map((m) =>
      percentiles ? `${m} (ns)` : points[0]?.unit || "Value",
    ),
  ];
  $("#results-head").replaceChildren(
    ...headers.map((label, i) => {
      const th = document.createElement("th");
      th.scope = "col";
      th.textContent = label;
      if (i === 2) th.id = "value-heading";
      return th;
    }),
  );
  const rows = points.map((p, i) => {
    const row = document.createElement("tr");
    const name = textCell(percentiles ? `#${i + 1} ${p.library}` : p.library);
    const dot = document.createElement("span");
    dot.className = "library-dot";
    dot.style.background = color(p.library);
    name.prepend(dot);
    row.append(
      name,
      textCell(
        benchmarkData.context.libraries.find((l) => l.name === p.library)
          ?.version || "Not recorded",
      ),
      ...metrics.map((m) =>
        textCell(formatValue(percentiles ? p.quantiles[m] : p.value, p.unit)),
      ),
    );
    return row;
  });
  const expected = benchmarkData.context.libraries.filter((l) =>
    state.category === "writer"
      ? l.role.toLowerCase().includes("writer")
      : state.category === "memory"
        ? l.role.includes("Reader") && l.language !== "Go"
        : l.role.includes("Reader"),
  );
  for (const lib of expected.filter(
    (l) => !points.some((p) => p.library === l.name),
  )) {
    const row = document.createElement("tr");
    row.append(
      textCell(lib.name),
      textCell(lib.version),
      ...metrics.map(() => textCell("Unavailable")),
    );
    rows.push(row);
  }
  $("#results-body").replaceChildren(...rows);
  if ($("#benchmarks").hidden) return;
  chart ??= echarts.init($("#benchmark-chart"), null, { renderer: "svg" });
  const css = getComputedStyle(document.documentElement),
    muted = css.getPropertyValue("--muted"),
    text = css.getPropertyValue("--text"),
    line = css.getPropertyValue("--line"),
    surface = css.getPropertyValue("--surface");
  const mobile = window.innerWidth < 560;
  chart.setOption(
    {
      animation: !reducedMotion,
      animationDuration: 350,
      aria: { enabled: true },
      grid: {
        left: mobile ? (percentiles ? 140 : 133) : percentiles ? 205 : 180,
        right: mobile ? (percentiles ? 80 : 65) : 120,
        top: percentiles ? 36 : 20,
        bottom: 38,
      },
      tooltip: {
        trigger: "item",
        confine: true,
        backgroundColor: surface,
        borderColor: line,
        textStyle: { color: text, fontFamily: "monospace" },
        formatter: (params) => {
          const p = points[params.dataIndex];
          return percentiles
            ? `#${params.dataIndex + 1} ${p.library}<br/>${metrics.map((m) => `${m}: ${formatValue(p.quantiles[m], p.unit)}`).join("<br/>")}`
            : `${p.library}<br/>${formatValue(p.value, p.unit)}`;
        },
      },
      xAxis: {
        type: "value",
        min: 0,
        splitNumber: mobile ? 2 : 4,
        axisLabel: { color: muted, fontSize: 10, hideOverlap: true },
        splitLine: { lineStyle: { color: line, type: "dashed" } },
        name: points[0]?.unit,
        nameLocation: "end",
        nameTextStyle: { color: muted, fontSize: 9 },
        axisLine: { show: false },
      },
      yAxis: {
        type: "category",
        inverse: true,
        data: points.map((p, i) =>
          percentiles ? `#${i + 1} ${p.library}` : p.library,
        ),
        axisLabel: {
          color: text,
          fontFamily: "monospace",
          fontSize: mobile ? 9 : 11,
        },
        axisTick: { show: false },
        axisLine: { show: false },
      },
      series: percentiles
        ? [percentileSeries(points, text, surface, mobile)]
        : [
            {
              type: "bar",
              barWidth: 22,
              showBackground: true,
              backgroundStyle: { color: line, opacity: 0.3, borderRadius: 3 },
              data: points.map((p) => ({
                value: p.value,
                itemStyle: {
                  color: color(p.library),
                  borderRadius: [0, 3, 3, 0],
                },
              })),
              label: {
                show: true,
                position: "right",
                color: text,
                fontFamily: "monospace",
                fontSize: mobile ? 9 : 11,
                formatter: (p) =>
                  new Intl.NumberFormat("en", {
                    maximumFractionDigits: 2,
                  }).format(p.value),
              },
            },
          ],
    },
    true,
  );
}
let mobileChart = window.innerWidth < 560;
new ResizeObserver(() => {
  if (mobileChart !== window.innerWidth < 560) {
    mobileChart = !mobileChart;
    renderChart();
  }
  chart?.resize();
}).observe($("#benchmark-chart"));
function showExample() {
  const example = examples.find((e) => e.id === state.example);
  if (!example) return;
  $("#example-description").textContent = example.description;
  $("#example-filename").textContent = `examples/${example.id}.rs`;
  $("#example-code").innerHTML = hljs.highlight(example.code, {
    language: "rust",
  }).value;
  $("#example-command").textContent = `cargo run --example ${example.id}`;
  $("#example-source").href = example.source;
  $$("[data-example]").forEach((el) => {
    const on = el.dataset.example === state.example;
    el.classList.toggle("active", on);
    el.setAttribute("aria-pressed", String(on));
  });
}
function showContext() {
  const c = benchmarkData.context;
  const version = c.libraries.find((l) => l.name === "libmaxminddb-rs").version;
  $("#summary-throughput").textContent =
    benchmarkData.charts
      .find((c) => c.id === "throughput-ipv4-random")
      ?.points.find((p) => p.library === "libmaxminddb-rs")?.value ??
    "Unavailable";
  $("#summary-context").textContent =
    `${c.architecture} · ${c.measurementDate || "Archived"} · libmaxminddb-rs ${version} · ${c.cpu}. ${c.notes}`;
  $("#snapshot-label").textContent = c.measurementDate
    ? "◷ MEASURED SNAPSHOT"
    : "◷ ARCHIVED SNAPSHOT";
  $("#snapshot-context").textContent =
    `${c.architecture} · ${c.measurementDate || c.sourceDate} · Rust ${c.rust} · libmaxminddb-rs ${version}`;
  $("#machine-label").textContent = c.cpu;
  $("#measurement-notes").textContent = c.notes;
  $("#compiler-note").textContent = c.compilerNote || "";
  const fields = [
    ["Architecture", c.architecture],
    ["Processor", c.cpu],
    ["Available CPUs", c.availableCpus || "Not recorded"],
    ["Platform", c.os],
    ["Rust toolchain", c.rust],
    ["Go toolchain", c.go],
    ["C compiler", c.cCompiler || "Not recorded"],
    ["Fixtures", c.dataset],
    ["Database sizes", "1K to 5M entries"],
    ["Measurement date", c.measurementDate || "Not recorded"],
    ["Benchmarked commit", c.sourceCommit.slice(0, 12)],
  ];
  $("#environment-details").replaceChildren(
    ...fields.flatMap(([label, value]) => {
      const dt = document.createElement("dt"),
        dd = document.createElement("dd");
      dt.textContent = label;
      dd.textContent = value;
      return [dt, dd];
    }),
  );
  $("#provenance-link").href =
    c.sourceExport ||
    `https://github.com/0x00F6/libmaxminddb-rs/tree/${c.sourceCommit}/benchmarks/charts`;
  if (c.workflowRunUrl) {
    $("#run-link").href = c.workflowRunUrl;
    $("#run-link").hidden = false;
  }
  if (c.report) {
    $("#report-link").href = `${import.meta.env.BASE_URL}${c.report}`;
    $("#report-link").hidden = false;
  }
  if (c.rawResults) {
    $("#raw-link").href = `${import.meta.env.BASE_URL}${c.rawResults}`;
    $("#raw-link").hidden = false;
  }
}
function selectArchitecture(id) {
  benchmarkData = benchmarkSets.get(id);
  if (!benchmarkData) return;
  $("#architecture-select").value = id;
  const dataset = datasets.find((d) => d.id === id);
  $("#export-data").href = `${import.meta.env.BASE_URL}data/${dataset.file}`;
  $("#export-data").download = `mmdb-benchmarks-${id}.json`;
  $("#libraries-body").replaceChildren(
    ...benchmarkData.context.libraries.map((l) => {
      const row = document.createElement("tr");
      row.append(...[l.name, l.version, l.language, l.role].map(textCell));
      return row;
    }),
  );
  showContext();
  controls(false);
}
$("#architecture-select").addEventListener("change", (e) => {
  selectArchitecture(e.target.value);
  const url = new URL(location.href);
  url.searchParams.set("arch", e.target.value);
  history.replaceState(null, "", url);
  try {
    localStorage.setItem("mmdb-architecture", e.target.value);
  } catch {}
});
async function fetchData(file) {
  const response = await fetch(`${import.meta.env.BASE_URL}data/${file}`);
  if (!response.ok) throw new Error(`Unable to load ${file}`);
  return response.json();
}
async function loadData() {
  try {
    [datasets, examples] = await Promise.all([
      fetchData("benchmark-datasets.json"),
      fetchData("examples.json"),
    ]);
    await Promise.all(
      datasets.map(async (dataset) => {
        benchmarkSets.set(dataset.id, await fetchData(dataset.file));
      }),
    );
    $("#architecture-select").replaceChildren(
      ...datasets.map((d) => option(d.id, d.label)),
    );
    let preferred;
    try {
      preferred = localStorage.getItem("mmdb-architecture");
    } catch {}
    const requested = new URLSearchParams(location.search).get("arch");
    if (benchmarkSets.has(requested)) preferred = requested;
    selectArchitecture(
      benchmarkSets.has(preferred) ? preferred : datasets[0].id,
    );
    $("#example-tabs").replaceChildren(
      ...examples.map((e) => {
        const b = document.createElement("button");
        b.textContent = e.label;
        b.dataset.example = e.id;
        b.addEventListener("click", () => {
          state.example = e.id;
          showExample();
        });
        return b;
      }),
    );
    controls();
    showExample();
  } catch (error) {
    console.error(error);
    $("#load-error").hidden = false;
  }
}
loadData();
