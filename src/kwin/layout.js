// Window Layout key: sets the columns of the KWin tile layout on the active
// screen and current desktop. Run inside KWin's script engine through
// org.kde.KWin /Scripting; the plugin prepends `const CONFIG = {...};`:
//   { mode: "cycle" | "set", target: "<layout id>",
//     layouts: [{ id, widths: [fractions summing to 1] }], tolerance }
// Plain JavaScript: no imports. Tests: tests/layout-script.test.mjs.

// Tile.LayoutDirection.Horizontal in KWin.
const HORIZONTAL = 1;
// Widths this close to a layout's are that layout, not drift from it.
const EXACT = 0.002;
// Two centres this close are a tie (the left column wins).
const TIE = 1e-6;
// Upper bound on any loop that asks KWin to change the tiles, so a KWin that
// does not do what we expect cannot hang the compositor.
const MAX_STEPS = 32;
// Deepest tile tree walked when collecting windows.
const MAX_DEPTH = 16;

/** The total difference from `widths` to `layout`, or null if any column is further than `limit`. */
function distance(layout, widths, limit) {
	if (layout.widths.length !== widths.length) return null;
	let total = 0;
	for (let i = 0; i < widths.length; i++) {
		const diff = Math.abs(layout.widths[i] - widths[i]);
		if (diff > limit) return null;
		total += diff;
	}
	return total;
}

/** Index of the layout nearest to `widths` with every column within `limit`, or -1. */
function closest(layouts, widths, limit) {
	let best = -1;
	let bestTotal = Infinity;
	for (let i = 0; i < layouts.length; i++) {
		const total = distance(layouts[i], widths, limit);
		if (total !== null && total < bestTotal) {
			best = i;
			bestTotal = total;
		}
	}
	return best;
}

/**
 * The layout to apply. Set mode: the target (null if the id is unknown).
 * Cycle mode: the layout after the one in use; the drifted-from layout if the
 * columns are only close to it; otherwise the first.
 */
function pickTarget(config, currentWidths) {
	const layouts = config.layouts;
	if (config.mode === "set") {
		for (const layout of layouts) {
			if (layout.id === config.target) return layout;
		}
		return null;
	}
	const exact = closest(layouts, currentWidths, EXACT);
	if (exact >= 0) return layouts[(exact + 1) % layouts.length];
	const drifted = closest(layouts, currentWidths, config.tolerance);
	if (drifted >= 0) return layouts[drifted];
	return layouts[0];
}

/** Indexes into `centres` (left to right), the one nearest `centre` first; ties go left. */
function nearestFirst(centre, centres) {
	const remaining = [];
	for (let i = 0; i < centres.length; i++) remaining.push(i);
	const order = [];
	while (remaining.length > 0) {
		let best = 0;
		for (let k = 1; k < remaining.length; k++) {
			const diff = Math.abs(centres[remaining[k]] - centre);
			if (diff < Math.abs(centres[remaining[best]] - centre) - TIE) best = k;
		}
		order.push(remaining.splice(best, 1)[0]);
	}
	return order;
}

/** True if the root is empty or a horizontal row of leaf columns. */
function isFlatRow(root) {
	if (root.tiles.length === 0) return true;
	if (root.layoutDirection !== HORIZONTAL) return false;
	for (const tile of root.tiles) {
		if (tile.tiles.length > 0) return false;
	}
	return true;
}

/**
 * Every window in a tile under `tile`, with its tile's horizontal centre (0..1), copied out.
 * Throws if the tree is deeper than MAX_DEPTH, before anything has changed.
 */
function collectWindows(tile, out, depth) {
	if (depth > MAX_DEPTH) throw new Error("the tile tree is deeper than " + MAX_DEPTH + ", left as it is");
	const windows = tile.windows;
	if (windows.length > 0) {
		// relativeGeometry is live in KWin: read the numbers now, before anything moves.
		const geometry = tile.relativeGeometry;
		const centre = geometry.x + geometry.width / 2;
		for (const window of windows) out.push({ window, centre });
	}
	for (const child of tile.tiles) collectWindows(child, out, depth + 1);
	return out;
}

/** Removes every child of the root. */
function clear(root) {
	for (let step = 0; step < MAX_STEPS && root.tiles.length > 0; step++) {
		const before = root.tiles.length;
		root.tiles[before - 1].remove();
		if (root.tiles.length >= before) break;
	}
}

/** Adds or drops columns at the right until the root has `count`. */
function resize(root, count) {
	for (let step = 0; step < MAX_STEPS && root.tiles.length !== count; step++) {
		const n = root.tiles.length;
		if (n === 0) root.split(HORIZONTAL);
		else if (n < count) root.tiles[n - 1].split(HORIZONTAL);
		else root.tiles[n - 1].remove();
	}
}

/** Asks each column to take its width, left to right. */
function writeWidths(columns, widths) {
	let x = 0;
	for (let i = 0; i < widths.length; i++) {
		columns[i].relativeGeometry = { x: x, y: 0, width: widths[i], height: 1 };
		x += widths[i];
	}
}

/** The widths the columns actually have, copied out. */
function readWidths(columns) {
	const widths = [];
	for (const column of columns) widths.push(column.relativeGeometry.width);
	return widths;
}

/** True if some column is further than EXACT from its target width. */
function differs(actual, target) {
	for (let i = 0; i < target.length; i++) {
		if (Math.abs(actual[i] - target[i]) > EXACT) return true;
	}
	return false;
}

function formatWidths(widths) {
	const out = [];
	for (const width of widths) out.push(String(Number(width.toFixed(3))));
	return out.join(", ");
}

/**
 * Makes the root a row of columns of these widths. KWin refuses a write that
 * would squeeze a neighbour to its minimum without saying so, and a refused
 * write can go through once its neighbour has moved: so write, read back, and
 * write once more if they differ. A layout that still differs is reported.
 */
function rebuild(root, widths) {
	if (!isFlatRow(root)) clear(root);
	resize(root, widths.length);
	if (root.tiles.length !== widths.length || !isFlatRow(root)) {
		throw new Error("could not make " + widths.length + " columns, the root has " + root.tiles.length);
	}
	const columns = [];
	for (const tile of root.tiles) columns.push(tile);
	writeWidths(columns, widths);
	if (!differs(readWidths(columns), widths)) return;
	writeWidths(columns, widths);
	const actual = readWidths(columns);
	if (differs(actual, widths)) {
		console.error(
			"opendeck-utilities layout: wanted columns " +
				formatWidths(widths) +
				", got " +
				formatWidths(actual) +
				" (a window may be too wide for its column to shrink)",
		);
	}
}

/**
 * The tiles to try for a window centred at `centre`, best first: the root's
 * leaf columns, nearest by their actual geometry first; if it has none, the
 * first leaf under it (the root itself if it has no children).
 */
function homes(root, centre) {
	const columns = [];
	const centres = [];
	for (const tile of root.tiles) {
		if (tile.tiles.length > 0) continue;
		const geometry = tile.relativeGeometry;
		columns.push(tile);
		centres.push(geometry.x + geometry.width / 2);
	}
	if (columns.length > 0) {
		const order = [];
		for (const i of nearestFirst(centre, centres)) order.push(columns[i]);
		return order;
	}
	let leaf = root;
	for (let depth = 0; depth < MAX_DEPTH && leaf.tiles.length > 0; depth++) leaf = leaf.tiles[0];
	return [leaf];
}

/** True if `tile` took `window`; a throw counts as a refusal. */
function tryManage(tile, window) {
	try {
		return tile.manage(window) !== false;
	} catch (error) {
		return false;
	}
}

function describe(window) {
	try {
		return String(window.resourceClass || window.caption || "a window");
	} catch (error) {
		return "a window";
	}
}

/** Puts each window in the best tile that takes it; reports the ones none would. */
function place(root, entries) {
	const lost = [];
	for (const entry of entries) {
		let tiles;
		try {
			tiles = homes(root, entry.centre);
		} catch (error) {
			tiles = [root];
		}
		let tiled = false;
		for (const tile of tiles) {
			if (tryManage(tile, entry.window)) {
				tiled = true;
				break;
			}
		}
		if (!tiled) lost.push(describe(entry.window));
	}
	if (lost.length > 0) console.error("opendeck-utilities layout: could not tile " + lost.join(", "));
}

/**
 * Makes the root a row of columns of these widths and puts every tiled window
 * back in the nearest one. Whatever fails once the windows are collected, they
 * are all put back in a tile before the error goes on to the caller.
 */
function apply(root, widths) {
	const placed = collectWindows(root, [], 0);
	try {
		rebuild(root, widths);
	} catch (error) {
		place(root, placed);
		throw error;
	}
	place(root, placed);
}

/** The column widths in use, or [] if the root is not a row of columns. */
function currentWidths(root) {
	if (!isFlatRow(root)) return [];
	const widths = [];
	for (const tile of root.tiles) widths.push(tile.relativeGeometry.width);
	return widths;
}

function main() {
	try {
		const root = workspace.rootTile(workspace.activeScreen, workspace.currentDesktop);
		if (!root) return;
		const target = pickTarget(CONFIG, currentWidths(root));
		if (!target) {
			console.error("opendeck-utilities layout: unknown layout " + CONFIG.target);
			return;
		}
		apply(root, target.widths);
	} catch (error) {
		console.error("opendeck-utilities layout: " + error);
	}
}

if (typeof workspace !== "undefined" && typeof CONFIG !== "undefined") {
	main();
}
