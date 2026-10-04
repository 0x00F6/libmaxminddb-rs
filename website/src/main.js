import "./style.css";
import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
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
    ["lookup", "Lookup p99"],
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
          ? `lookup-latency-${f}`
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
function renderChart() {
  if (!benchmarkData) return;
  const data = selectChart();
  if (!data) return;
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
    concurrent:
      "4, 8 and 16-worker measurements on 4 available vCPUs: 8/16 workers oversubscribe the runner. The 1-worker export mixes fallback scenarios and is excluded; this chart does not claim linear speedup.",
    memory:
      "Whole-process resident memory in mmap mode, including the prepared tree and runtime. Three isolated processes per size in the comparison protocol. Go reader RSS is not part of this export.",
    writer:
      "Same database sizes for the Rust writer and Go mmdbwriter. Throughput, total build duration and peak resident memory describe different costs.",
  };
  $("#metric-note").textContent = notes[state.category];
  $("#value-heading").textContent = points[0]?.unit || "Value";
  const rows = points.map((p) => {
    const row = document.createElement("tr");
    const name = textCell(p.library);
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
      textCell(formatValue(p.value, p.unit)),
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
      textCell("Unavailable"),
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
        left: mobile ? 133 : 180,
        right: mobile ? 65 : 120,
        top: 20,
        bottom: 38,
      },
      tooltip: {
        trigger: "item",
        backgroundColor: surface,
        borderColor: line,
        textStyle: { color: text, fontFamily: "monospace" },
        formatter: (params) =>
          `${points[params.dataIndex].library}<br/>${formatValue(points[params.dataIndex].value, points[params.dataIndex].unit)}`,
      },
      xAxis: {
        type: "value",
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
        data: points.map((p) => p.library),
        axisLabel: {
          color: text,
          fontFamily: "monospace",
          fontSize: mobile ? 9 : 11,
        },
        axisTick: { show: false },
        axisLine: { show: false },
      },
      series: [
        {
          type: "bar",
          barWidth: 22,
          showBackground: true,
          backgroundStyle: { color: line, opacity: 0.3, borderRadius: 3 },
          data: points.map((p) => ({
            value: p.value,
            itemStyle: { color: color(p.library), borderRadius: [0, 3, 3, 0] },
          })),
          label: {
            show: true,
            position: "right",
            color: text,
            fontFamily: "monospace",
            fontSize: mobile ? 9 : 11,
            formatter: (p) =>
              new Intl.NumberFormat("en", { maximumFractionDigits: 2 }).format(
                p.value,
              ),
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
    `${c.measurementDate || "Archived"} · libmaxminddb-rs ${version} · ${c.cpu}. ${c.notes}`;
  $("#snapshot-label").textContent = c.measurementDate
    ? "◷ MEASURED SNAPSHOT"
    : "◷ ARCHIVED SNAPSHOT";
  $("#snapshot-context").textContent =
    `${c.measurementDate || c.sourceDate} · Rust ${c.rust} · libmaxminddb-rs ${version}`;
  $("#machine-label").textContent = c.cpu;
  $("#measurement-notes").textContent = c.notes;
  $("#compiler-note").textContent = c.compilerNote || "";
  const fields = [
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
async function loadData() {
  try {
    [benchmarkData, examples] = await Promise.all(
      ["benchmarks", "examples"].map(async (name) => {
        const r = await fetch(`${import.meta.env.BASE_URL}data/${name}.json`);
        if (!r.ok) throw new Error(`Unable to load ${name}`);
        return r.json();
      }),
    );
    $("#libraries-body").replaceChildren(
      ...benchmarkData.context.libraries.map((l) => {
        const row = document.createElement("tr");
        row.append(...[l.name, l.version, l.language, l.role].map(textCell));
        return row;
      }),
    );
    showContext();
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
