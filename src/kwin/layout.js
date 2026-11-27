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

/** Index of the column (of these widths, left to right) whose centre is nearest `centre`; ties go left. */
function nearestColumn(centre, widths) {
	let best = 0;
	let bestDiff = Infinity;
	let x = 0;
	for (let i = 0; i < widths.length; i++) {
		const diff = Math.abs(x + widths[i] / 2 - centre);
		if (diff < bestDiff - TIE) {
			best = i;
			bestDiff = diff;
		}
		x += widths[i];
	}
	return best;
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

/** Every window in a tile under `tile`, with its tile's horizontal centre (0..1), copied out. */
function collectWindows(tile, out, depth) {
	if (depth > MAX_DEPTH) return out;
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

/** Makes the root a row of columns of these widths and puts every tiled window back in the nearest one. */
function apply(root, widths) {
	const placed = collectWindows(root, [], 0);
	if (!isFlatRow(root)) clear(root);
	resize(root, widths.length);
	if (root.tiles.length !== widths.length || !isFlatRow(root)) {
		throw new Error("could not make " + widths.length + " columns, the root has " + root.tiles.length);
	}
	const columns = [];
	for (const tile of root.tiles) columns.push(tile);
	let x = 0;
	for (let i = 0; i < widths.length; i++) {
		columns[i].relativeGeometry = { x: x, y: 0, width: widths[i], height: 1 };
		x += widths[i];
	}
	for (const entry of placed) {
		columns[nearestColumn(entry.centre, widths)].manage(entry.window);
	}
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
