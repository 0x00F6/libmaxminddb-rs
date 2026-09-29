// Sort every report table in place. Missing and unsupported measurements stay last.
(() => {
  const textCollator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  const textHeaders = /compiler|version|library|language|role|repository|scenario|metric|implementation|family|\bapi\b|benchmark|operation|key|strategy|status|category|fichier|statut|catégorie|stratégie|clé|bibliothèque/i;

  function cellAt(row, column) {
    let position = 0;
    for (const cell of row.cells) {
      const end = position + cell.colSpan;
      if (column >= position && column < end) return cell;
      position = end;
    }
    return null;
  }

  function numberIn(cell) {
    if (!cell) return null;
    const explicit = cell.dataset.sortValue;
    if (explicit !== undefined) {
      const value = Number(explicit);
      return Number.isFinite(value) ? value : null;
    }
    const text = cell.textContent.replace(/\u00a0/g, " ").trim();
    if (!text || /^(—|–|n\/a|unsupported|failed|⚠️)/i.test(text)) return null;
    const match = /[-+]?\d+(?:[.,]\d+)?(?:e[-+]?\d+)?/i.exec(text);
    if (!match) return null;
    const value = Number(match[0].replace(",", "."));
    if (!Number.isFinite(value)) return null;
    const suffix = text.slice(match.index + match[0].length).trim().toLowerCase();
    if (/^(ns|nanosecond)/.test(suffix)) return value;
    if (/^(µs|μs|us|microsecond)/.test(suffix)) return value * 1e3;
    if (/^(ms|millisecond)/.test(suffix)) return value * 1e6;
    if (/^(s|second)/.test(suffix)) return value * 1e9;
    if (/^gib/.test(suffix)) return value * 1024 ** 3;
    if (/^mib/.test(suffix)) return value * 1024 ** 2;
    if (/^kib/.test(suffix)) return value * 1024;
    if (/^g(?:\s*ops\/s|\b)/.test(suffix)) return value * 1e9;
    if (/^m(?:\s*ops\/s|\b)/.test(suffix)) return value * 1e6;
    if (/^k(?:\s*ops\/s|\b)/.test(suffix)) return value * 1e3;
    return value;
  }

  function preferredDirection(label, numeric) {
    if (!numeric) return "asc";
    return /throughput|ops\/s|débit|insert/i.test(label) ? "desc" : "asc";
  }

  for (const table of document.querySelectorAll("table")) {
    const body = table.tBodies[0];
    const headers = Array.from(table.tHead?.rows[0]?.cells || []);
    if (!body || !headers.length) continue;
    const originalRows = Array.from(body.rows);
    if (!originalRows.length) continue;
    const numeric = headers.map((header, column) => {
      if (originalRows.some((row) => cellAt(row, column)?.dataset.sortValue !== undefined)) {
        return true;
      }
      if (textHeaders.test(header.textContent)) return false;
      const cells = originalRows.map((row) => cellAt(row, column));
      return cells.filter((cell) => numberIn(cell) !== null).length >= Math.ceil(cells.length / 2);
    });
    let activeColumn = -1;
    let activeDirection = "asc";

    function sort(column, direction) {
      const rows = Array.from(body.rows).map((row, index) => ({ row, index }));
      rows.sort((a, b) => {
        const left = cellAt(a.row, column);
        const right = cellAt(b.row, column);
        const leftNumber = numberIn(left);
        const rightNumber = numberIn(right);
        const leftMissing = !left || /^(—|–|n\/a|unsupported|failed|⚠️)/i.test(left.textContent.trim());
        const rightMissing = !right || /^(—|–|n\/a|unsupported|failed|⚠️)/i.test(right.textContent.trim());
        if (leftMissing !== rightMissing) return leftMissing ? 1 : -1;
        if (numeric[column] && (leftNumber === null) !== (rightNumber === null)) {
          return leftNumber === null ? 1 : -1;
        }
        let order;
        if (numeric[column] && leftNumber !== null && rightNumber !== null) {
          order = Math.sign(leftNumber - rightNumber);
        } else {
          order = textCollator.compare(left?.textContent.trim() || "", right?.textContent.trim() || "");
        }
        return (direction === "asc" ? order : -order) || a.index - b.index;
      });
      body.append(...rows.map(({ row }) => row));
      activeColumn = column;
      activeDirection = direction;
      headers.forEach((header, index) => {
        header.setAttribute("aria-sort", index === column ? (direction === "asc" ? "ascending" : "descending") : "none");
        header.querySelector(".sort-indicator").textContent = index === column ? (direction === "asc" ? "▲" : "▼") : "↕";
      });
    }

    headers.forEach((header, column) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "sort-button";
      while (header.firstChild) button.append(header.firstChild);
      const indicator = document.createElement("span");
      indicator.className = "sort-indicator";
      indicator.setAttribute("aria-hidden", "true");
      indicator.textContent = "↕";
      button.append(indicator);
      header.append(button);
      button.addEventListener("click", () => {
        const direction = activeColumn === column
          ? (activeDirection === "asc" ? "desc" : "asc")
          : preferredDirection(button.textContent, numeric[column]);
        sort(column, direction);
      });
    });

    const [columnText, directionText] = (table.dataset.defaultSort || "0:asc").split(":");
    const column = Number(columnText);
    sort(Number.isInteger(column) && column >= 0 && column < headers.length ? column : 0,
      directionText === "desc" ? "desc" : "asc");
  }
})();
