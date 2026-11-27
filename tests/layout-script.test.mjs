// Tests for the KWin script in src/kwin/layout.js, run against a fake `workspace`.
// Run with: node --test tests/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("../src/kwin/layout.js", import.meta.url), "utf8");

const HORIZONTAL = 1;
const VERTICAL = 2;

const LAYOUTS = [
	{ id: "thirds", widths: [1 / 3, 1 / 3, 1 / 3] },
	{ id: "quarters", widths: [0.25, 0.5, 0.25] },
	{ id: "fifths", widths: [0.2, 0.6, 0.2] },
];

/** The table plus a two-column layout, for the tests of the column machinery. */
const WITH_TWO = [...LAYOUTS, { id: "two", widths: [0.5, 0.5] }];

function config(mode = "cycle", target = "quarters") {
	return { mode, target, layouts: LAYOUTS, tolerance: 0.08 };
}

/**
 * A tile that behaves like KWin 6.8's CustomTile as far as the script uses it
 * (checked live on 2026-11-27): `split(1)` on a column in a horizontal row adds
 * a sibling after it and halves it; `split` on a leaf root makes two children;
 * `remove()` gives the space and the windows to the left neighbour (or the right
 * one for the first column); `relativeGeometry` reads are live.
 *
 * Writing `relativeGeometry` on a column moves the edges it shares with its
 * neighbours (the outer edges of the row stay put), and silently does nothing
 * when the column or a neighbour would end up at or below its `minWidth`.
 * `manage(window)` returns false for a window named in the tile's `refuses`
 * and throws for a window with `manageThrows`. A tile with `strands` set
 * untiles its windows when removed, the worst KWin could do.
 */
class FakeTile {
	constructor(parent, geom) {
		this.parent = parent;
		this.tiles = [];
		this.windows = [];
		this.layoutDirection = HORIZONTAL;
		this.geom = { ...geom };
		this.minWidth = 0;
		this.refuses = [];
		this.strands = false;
		this.writes = 0;
	}

	get relativeGeometry() {
		const tile = this;
		return {
			get x() {
				return tile.geom.x;
			},
			get y() {
				return tile.geom.y;
			},
			get width() {
				return tile.geom.width;
			},
			get height() {
				return tile.geom.height;
			},
		};
	}

	set relativeGeometry(r) {
		this.writes++;
		const parent = this.parent;
		if (!parent || parent.layoutDirection !== HORIZONTAL) {
			this.geom = { x: r.x, y: r.y, width: r.width, height: r.height };
			return;
		}
		const i = parent.tiles.indexOf(this);
		const before = parent.tiles[i - 1];
		const after = parent.tiles[i + 1];
		const g = this.geom;
		const left = before ? r.x : g.x;
		const right = after ? r.x + r.width : g.x + g.width;
		const tooNarrow = (tile, width) => width <= tile.minWidth;
		if (tooNarrow(this, right - left)) return;
		if (before && tooNarrow(before, left - before.geom.x)) return;
		if (after && tooNarrow(after, after.geom.x + after.geom.width - right)) return;
		if (before) before.geom = { ...before.geom, width: left - before.geom.x };
		if (after) after.geom = { ...after.geom, x: right, width: after.geom.x + after.geom.width - right };
		this.geom = { x: left, y: parent.geom.y, width: right - left, height: parent.geom.height };
	}

	split(direction) {
		assert.equal(direction, HORIZONTAL, "the script only splits horizontally");
		const parent = this.parent;
		const g = this.geom;
		if (parent && (parent.tiles.length < 2 || parent.layoutDirection === direction)) {
			parent.layoutDirection = direction;
			const half = g.width / 2;
			this.geom = { ...g, width: half };
			const sibling = new FakeTile(parent, { x: g.x + half, y: g.y, width: half, height: g.height });
			parent.tiles.splice(parent.tiles.indexOf(this) + 1, 0, sibling);
			return [this, sibling];
		}
		assert.equal(this.tiles.length, 0, "split with children is not modelled");
		this.layoutDirection = direction;
		const a = new FakeTile(this, { x: g.x, y: g.y, width: g.width / 2, height: g.height });
		const b = new FakeTile(this, { x: g.x + g.width / 2, y: g.y, width: g.width / 2, height: g.height });
		this.tiles = [a, b];
		for (const w of [...this.windows]) a.take(w);
		return [a, b];
	}

	remove() {
		const parent = this.parent;
		if (!parent) return;
		const i = parent.tiles.indexOf(this);
		parent.tiles.splice(i, 1);
		const neighbour = parent.tiles[i - 1] ?? parent.tiles[i] ?? parent;
		if (neighbour !== parent && parent.layoutDirection === HORIZONTAL) {
			if (neighbour === parent.tiles[i - 1]) neighbour.geom.width += this.geom.width;
			else neighbour.geom = { ...neighbour.geom, x: this.geom.x, width: neighbour.geom.width + this.geom.width };
		}
		let home = neighbour;
		while (home.tiles.length > 0) home = home.tiles[0];
		for (const w of windowsUnder(this)) {
			if (this.strands) untile(w);
			else home.take(w);
		}
		this.parent = null;
	}

	manage(window) {
		manageCalls.push(window.name);
		if (window.manageThrows) throw new Error("manage failed for " + window.name);
		if (this.refuses.includes(window.name)) return false;
		this.take(window);
		return true;
	}

	take(window) {
		untile(window);
		this.windows.push(window);
		window.tile = this;
	}
}

let manageCalls = [];

function untile(window) {
	if (window.tile) window.tile.windows = window.tile.windows.filter((w) => w !== window);
	window.tile = null;
}

function windowsUnder(tile) {
	return [...tile.windows, ...tile.tiles.flatMap(windowsUnder)];
}

function tilesUnder(tile) {
	return [tile, ...tile.tiles.flatMap(tilesUnder)];
}

/** A root holding a horizontal row of columns with these widths. */
function row(widths) {
	const root = new FakeTile(null, { x: 0, y: 0, width: 1, height: 1 });
	let x = 0;
	for (const width of widths) {
		root.tiles.push(new FakeTile(root, { x, y: 0, width, height: 1 }));
		x += width;
	}
	return root;
}

function windowIn(name, tile) {
	const window = { name, resourceClass: name, tile: null };
	if (tile) tile.take(window);
	return window;
}

function widthsOf(root) {
	return root.tiles.map((t) => t.geom.width);
}

function assertColumns(root, expected) {
	assert.equal(root.layoutDirection, HORIZONTAL);
	assert.equal(root.tiles.length, expected.length, "column count");
	let x = 0;
	root.tiles.forEach((tile, i) => {
		assert.equal(tile.tiles.length, 0, `column ${i} is a leaf`);
		assert.ok(Math.abs(tile.geom.x - x) < 1e-9, `column ${i} x ${tile.geom.x} != ${x}`);
		assert.ok(Math.abs(tile.geom.width - expected[i]) < 1e-9, `column ${i} width ${tile.geom.width} != ${expected[i]}`);
		assert.equal(tile.geom.y, 0);
		assert.equal(tile.geom.height, 1);
		x += expected[i];
	});
}

/** The script's functions, loaded without running it against KWin. */
function load() {
	const context = { console: { error() {}, log() {} } };
	vm.runInNewContext(SOURCE, context);
	return context;
}

/** Runs the script the way KWin would, with `CONFIG` prepended. */
function run(cfg, root, extraWindows = []) {
	const errors = [];
	const workspace = {
		activeScreen: "DP-1",
		currentDesktop: "desktop-1",
		rootTile(screen, desktop) {
			assert.equal(screen, "DP-1");
			assert.equal(desktop, "desktop-1");
			return root;
		},
		windowList() {
			return [...(root ? windowsUnder(root) : []), ...extraWindows];
		},
	};
	const context = { workspace, console: { error: (...args) => errors.push(args.join(" ")), log() {} } };
	vm.runInNewContext(`const CONFIG = ${JSON.stringify(cfg)};\n${SOURCE}`, context);
	return errors;
}

const script = load();

test("cycle: thirds goes to quarters, quarters to fifths, fifths wraps to thirds", () => {
	assert.equal(script.pickTarget(config(), [1 / 3, 1 / 3, 1 / 3]).id, "quarters");
	assert.equal(script.pickTarget(config(), [0.25, 0.5, 0.25]).id, "fifths");
	assert.equal(script.pickTarget(config(), [0.2, 0.6, 0.2]).id, "thirds");
});

test("cycle: widths a hair off an exact layout still count as that layout", () => {
	assert.equal(script.pickTarget(config(), [0.3335, 0.333, 0.3335]).id, "quarters");
	assert.equal(script.pickTarget(config(), [0.2005, 0.599, 0.2005]).id, "thirds");
});

test("cycle: an exact layout is never mistaken for a neighbour within the tolerance", () => {
	for (const [i, layout] of LAYOUTS.entries()) {
		const next = LAYOUTS[(i + 1) % LAYOUTS.length];
		assert.equal(script.pickTarget(config(), layout.widths).id, next.id);
		for (const other of LAYOUTS) {
			if (other === layout) continue;
			const widest = Math.max(...layout.widths.map((w, k) => Math.abs(w - other.widths[k])));
			assert.ok(widest > 0.08, `${layout.id} is ${widest} from ${other.id}`);
		}
	}
});

test("cycle: drifted columns are restored to the nearest layout", () => {
	assert.equal(script.pickTarget(config(), [0.22, 0.56, 0.22]).id, "fifths");
	assert.equal(script.pickTarget(config(), [0.24, 0.52, 0.24]).id, "quarters");
	assert.equal(script.pickTarget(config(), [0.3, 0.38, 0.32]).id, "thirds");
	assert.equal(script.pickTarget(config(), [0.28, 0.45, 0.27]).id, "quarters");
});

test("cycle: 30/40/30 is within the tolerance of thirds only, so thirds is restored", () => {
	assert.equal(script.pickTarget(config(), [0.3, 0.4, 0.3]).id, "thirds");
});

test("cycle: a tie in the largest column difference goes to the earlier layout", () => {
	const two = { mode: "cycle", target: "a", tolerance: 0.2, layouts: [{ id: "a", widths: [0.4, 0.6] }, { id: "b", widths: [0.6, 0.4] }] };
	assert.equal(script.pickTarget(two, [0.5, 0.5]).id, "a");
	const reversed = { ...two, layouts: [...two.layouts].reverse() };
	assert.equal(script.pickTarget(reversed, [0.5, 0.5]).id, "b");
});

test("cycle: a screen with another column count goes to the first layout", () => {
	assert.equal(script.pickTarget(config(), [0.25, 0.25, 0.25, 0.25]).id, "thirds");
	assert.equal(script.pickTarget(config(), [0.5, 0.5]).id, "thirds");
	assert.equal(script.pickTarget(config(), [0.8, 0.2]).id, "thirds");
	assert.equal(script.pickTarget(config(), []).id, "thirds");
	assert.equal(script.pickTarget(config(), [1]).id, "thirds");
});

test("set: the target is returned whatever the current layout", () => {
	for (const current of [[0.5, 0.5], [0.25, 0.5, 0.25], [0.8, 0.2], []]) {
		assert.equal(script.pickTarget(config("set", "quarters"), current).id, "quarters");
		assert.equal(script.pickTarget(config("set", "thirds"), current).id, "thirds");
	}
});

test("nearestFirst orders columns by how near their centre is, ties go left", () => {
	const order = (centre, centres) => [...script.nearestFirst(centre, centres)];
	assert.deepEqual(order(0.1, [0.25, 0.75]), [0, 1]);
	assert.deepEqual(order(0.9, [0.25, 0.75]), [1, 0]);
	assert.deepEqual(order(0.5, [0.25, 0.75]), [0, 1]);
	assert.deepEqual(order(0.5, [1 / 6, 0.5, 5 / 6]), [1, 0, 2]);
	assert.deepEqual(order(5 / 6, [1 / 6, 0.5, 5 / 6]), [2, 1, 0]);
	assert.deepEqual(order(0.125, [0.125, 0.5, 0.875]), [0, 1, 2]);
	assert.deepEqual(order(0.5, []), []);
});

test("apply: three columns down to two keeps left left and right right; the middle goes left", () => {
	manageCalls = [];
	const root = row([1 / 3, 1 / 3, 1 / 3]);
	const left = windowIn("left", root.tiles[0]);
	const middle = windowIn("middle", root.tiles[1]);
	const right = windowIn("right", root.tiles[2]);
	script.apply(root, [0.5, 0.5]);
	assertColumns(root, [0.5, 0.5]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(middle.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[1]);
});

test("apply: 25/50/25 down to two columns sends the middle window left", () => {
	const root = row([0.25, 0.5, 0.25]);
	const middle = windowIn("middle", root.tiles[1]);
	const right = windowIn("right", root.tiles[2]);
	script.apply(root, [0.5, 0.5]);
	assertColumns(root, [0.5, 0.5]);
	assert.equal(middle.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[1]);
});

test("apply: four columns down to three keeps left left and right right", () => {
	const root = row([0.25, 0.25, 0.25, 0.25]);
	const windows = root.tiles.map((tile, i) => windowIn(`w${i}`, tile));
	script.apply(root, [1 / 3, 1 / 3, 1 / 3]);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
	assert.equal(windows[0].tile, root.tiles[0]);
	assert.equal(windows[3].tile, root.tiles[2]);
});

test("apply: 2 to 3 columns keeps left left and right right", () => {
	const root = row([0.5, 0.5]);
	const left = windowIn("left", root.tiles[0]);
	const right = windowIn("right", root.tiles[1]);
	script.apply(root, [1 / 3, 1 / 3, 1 / 3]);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[2]);
});

test("apply: same count only changes the widths and keeps every window in its column", () => {
	const root = row([1 / 3, 1 / 3, 1 / 3]);
	const windows = root.tiles.map((tile, i) => windowIn(`w${i}`, tile));
	const columns = [...root.tiles];
	script.apply(root, [0.25, 0.5, 0.25]);
	assertColumns(root, [0.25, 0.5, 0.25]);
	columns.forEach((column, i) => assert.equal(root.tiles[i], column));
	windows.forEach((w, i) => assert.equal(w.tile, root.tiles[i]));
});

test("apply: an empty root gets a row of columns", () => {
	const root = row([]);
	script.apply(root, [0.25, 0.5, 0.25]);
	assertColumns(root, [0.25, 0.5, 0.25]);
});

test("apply: a root whose column holds rows is rebuilt as a flat row", () => {
	const root = row([0.5, 0.5]);
	const nested = root.tiles[1];
	nested.layoutDirection = VERTICAL;
	nested.tiles = [
		new FakeTile(nested, { x: 0.5, y: 0, width: 0.5, height: 0.5 }),
		new FakeTile(nested, { x: 0.5, y: 0.5, width: 0.5, height: 0.5 }),
	];
	const left = windowIn("left", root.tiles[0]);
	const top = windowIn("top", nested.tiles[0]);
	const bottom = windowIn("bottom", nested.tiles[1]);
	script.apply(root, [1 / 3, 1 / 3, 1 / 3]);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(top.tile, root.tiles[2]);
	assert.equal(bottom.tile, root.tiles[2]);
});

test("apply: a vertical root is rebuilt as a horizontal row", () => {
	const root = new FakeTile(null, { x: 0, y: 0, width: 1, height: 1 });
	root.layoutDirection = VERTICAL;
	root.tiles = [
		new FakeTile(root, { x: 0, y: 0, width: 1, height: 0.5 }),
		new FakeTile(root, { x: 0, y: 0.5, width: 1, height: 0.5 }),
	];
	const top = windowIn("top", root.tiles[0]);
	const bottom = windowIn("bottom", root.tiles[1]);
	script.apply(root, [0.5, 0.5]);
	assertColumns(root, [0.5, 0.5]);
	// Both were centred at 0.5: a tie, so the left column.
	assert.equal(top.tile, root.tiles[0]);
	assert.equal(bottom.tile, root.tiles[0]);
});

test("main: a cycle press on two columns rebuilds them as thirds, windows keeping their sides", () => {
	const root = row([0.5, 0.5]);
	const left = windowIn("left", root.tiles[0]);
	const right = windowIn("right", root.tiles[1]);
	const errors = run(config(), root);
	assert.deepEqual(errors, []);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[2]);
});

test("main: a cycle press on four columns rebuilds them as thirds, windows keeping their sides", () => {
	const root = row([0.25, 0.25, 0.25, 0.25]);
	const left = windowIn("left", root.tiles[0]);
	const right = windowIn("right", root.tiles[3]);
	assert.deepEqual(run(config(), root), []);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[2]);
});

test("main: each cycle press moves one layout on and the third wraps round", () => {
	const root = row([1 / 3, 1 / 3, 1 / 3]);
	run(config(), root);
	assertColumns(root, [0.25, 0.5, 0.25]);
	run(config(), root);
	assertColumns(root, [0.2, 0.6, 0.2]);
	run(config(), root);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: drift is restored first, then the next press cycles on", () => {
	const root = row([0.28, 0.45, 0.27]);
	run(config(), root);
	assertColumns(root, [0.25, 0.5, 0.25]);
	run(config(), root);
	assertColumns(root, [0.2, 0.6, 0.2]);
	run(config(), root);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: 22/56/22 is restored to fifths, 24/52/24 to quarters", () => {
	const a = row([0.22, 0.56, 0.22]);
	run(config(), a);
	assertColumns(a, [0.2, 0.6, 0.2]);
	const b = row([0.24, 0.52, 0.24]);
	run(config(), b);
	assertColumns(b, [0.25, 0.5, 0.25]);
});

test("main: an unknown layout goes to the first", () => {
	const four = row([0.25, 0.25, 0.25, 0.25]);
	run(config(), four);
	assertColumns(four, [1 / 3, 1 / 3, 1 / 3]);
	const lopsided = row([0.8, 0.2]);
	run(config(), lopsided);
	assertColumns(lopsided, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: a root that is not a flat row counts as unknown and is rebuilt", () => {
	const root = new FakeTile(null, { x: 0, y: 0, width: 1, height: 1 });
	root.layoutDirection = VERTICAL;
	root.tiles = [
		new FakeTile(root, { x: 0, y: 0, width: 1, height: 0.5 }),
		new FakeTile(root, { x: 0, y: 0.5, width: 1, height: 0.5 }),
	];
	run(config(), root);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: set mode applies the target whatever the current layout", () => {
	for (const start of [[0.5, 0.5], [0.25, 0.5, 0.25], [0.8, 0.2], [0.25, 0.25, 0.25, 0.25], []]) {
		const root = row(start);
		run(config("set", "quarters"), root);
		assertColumns(root, [0.25, 0.5, 0.25]);
	}
});

test("main: a window not in any tile is not touched", () => {
	manageCalls = [];
	const root = row([0.5, 0.5]);
	const tiled = windowIn("tiled", root.tiles[1]);
	const floating = windowIn("floating", null);
	run(config("set", "thirds"), root, [floating]);
	assert.equal(floating.tile, null);
	assert.ok(!manageCalls.includes("floating"));
	assert.equal(tiled.tile, root.tiles[2]);
});

test("main: no root tile does nothing and does not throw", () => {
	assert.deepEqual(run(config(), null), []);
	assert.deepEqual(run(config(), undefined), []);
});

test("main: a failure is reported on console.error, not thrown", () => {
	const root = row([0.5, 0.5]);
	root.tiles[1].split = () => {
		throw new Error("boom");
	};
	const errors = run(config("set", "thirds"), root);
	assert.equal(errors.length, 1);
	assert.match(errors[0], /^opendeck-utilities layout: .*boom/);
});

/** A root whose second column holds two rows, with a window in each tile; every tile strands its windows when removed. */
function nestedRoot() {
	const root = row([0.5, 0.5]);
	const nested = root.tiles[1];
	nested.layoutDirection = VERTICAL;
	nested.tiles = [
		new FakeTile(nested, { x: 0.5, y: 0, width: 0.5, height: 0.5 }),
		new FakeTile(nested, { x: 0.5, y: 0.5, width: 0.5, height: 0.5 }),
	];
	const windows = [windowIn("left", root.tiles[0]), windowIn("top", nested.tiles[0]), windowIn("bottom", nested.tiles[1])];
	for (const tile of tilesUnder(root)) tile.strands = true;
	return { root, windows };
}

test("main: a nested root whose rebuild fails leaves every window tiled", () => {
	const failures = {
		throws() {
			throw new Error("split refused");
		},
		refuses() {
			return undefined;
		},
	};
	for (const [how, split] of Object.entries(failures)) {
		const { root, windows } = nestedRoot();
		root.split = split;
		const errors = run(config("set", "thirds"), root);
		for (const w of windows) assert.ok(w.tile, `${how}: ${w.name} is tiled`);
		assert.ok(
			errors.some((e) => /^opendeck-utilities layout: /.test(e)),
			`${how}: the failure is reported: ${errors}`,
		);
	}
});

test("main: a flat root whose split fails puts every window back in the nearest column", () => {
	const root = row([0.5, 0.5]);
	const left = windowIn("left", root.tiles[0]);
	const right = windowIn("right", root.tiles[1]);
	// The split is refused and, at worst, strands the windows.
	root.tiles[1].split = () => {
		untile(left);
		untile(right);
	};
	const errors = run(config("set", "thirds"), root);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[1]);
	assert.equal(errors.length, 1);
	assert.match(errors[0], /^opendeck-utilities layout: .*could not make 3 columns/);
});

test("main: a window whose manage throws does not stop the others", () => {
	manageCalls = [];
	const root = row([1 / 3, 1 / 3, 1 / 3]);
	const left = windowIn("left", root.tiles[0]);
	const bad = windowIn("bad", root.tiles[1]);
	bad.manageThrows = true;
	const right = windowIn("right", root.tiles[2]);
	const errors = run(config("set", "quarters"), root);
	assertColumns(root, [0.25, 0.5, 0.25]);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[2]);
	assert.ok(manageCalls.indexOf("right") > manageCalls.indexOf("bad"), "right was managed after bad failed");
	assert.equal(errors.length, 1);
	assert.match(errors[0], /^opendeck-utilities layout: .*bad/);
});

test("main: a column that refuses a window sends it to the next nearest column", () => {
	const root = row([0.5, 0.5]);
	const left = windowIn("left", root.tiles[0]);
	const right = windowIn("right", root.tiles[1]);
	root.tiles[1].refuses = ["right"];
	const errors = run({ ...config("set", "two"), layouts: WITH_TWO }, root);
	assert.equal(left.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[0]);
	assert.deepEqual(errors, []);
});

test("main: a window no column takes is reported", () => {
	const root = row([0.5, 0.5]);
	windowIn("left", root.tiles[0]);
	windowIn("right", root.tiles[1]);
	for (const column of root.tiles) column.refuses = ["right"];
	const errors = run({ ...config("set", "two"), layouts: WITH_TWO }, root);
	assert.equal(errors.length, 1);
	assert.match(errors[0], /^opendeck-utilities layout: .*right/);
});

test("apply: a tile tree deeper than the cap is left as it is", () => {
	manageCalls = [];
	const root = new FakeTile(null, { x: 0, y: 0, width: 1, height: 1 });
	let tile = root;
	for (let depth = 0; depth < 20; depth++) {
		const child = new FakeTile(tile, { x: 0, y: 0, width: 1, height: 1 });
		tile.tiles = [child];
		tile = child;
	}
	const deep = windowIn("deep", tile);
	const top = root.tiles[0];
	assert.throws(() => script.apply(root, [0.5, 0.5]), /deeper/);
	assert.deepEqual(root.tiles, [top]);
	assert.equal(deep.tile, tile);
	assert.deepEqual(manageCalls, []);
});

test("main: a write refused on the first pass is made on the second", () => {
	// Growing the first columns past their neighbours' far edges is refused
	// until a later column has moved those edges out of the way.
	const four = { mode: "set", target: "four", layouts: [{ id: "four", widths: [0.3, 0.3, 0.3, 0.1] }], tolerance: 0.08 };
	const root = row([0.1, 0.1, 0.1, 0.7]);
	const errors = run(four, root);
	assert.deepEqual(errors, []);
	assertColumns(root, [0.3, 0.3, 0.3, 0.1]);
	assert.equal(root.tiles[0].writes, 2, "the first column was written twice");
});

test("main: a column that cannot shrink is reported and windows go by the actual columns", () => {
	const root = row([0.2, 0.5, 0.3]);
	const a = windowIn("a", root.tiles[0]);
	const b = windowIn("b", root.tiles[1]);
	const c = windowIn("c", root.tiles[2]);
	// A window in the middle column is too wide for it to go below 0.6.
	root.tiles[1].minWidth = 0.6;
	const errors = run({ ...config("set", "two"), layouts: WITH_TWO }, root);
	assert.equal(root.tiles.length, 2);
	assert.deepEqual(widthsOf(root), [0.2, 0.8]);
	// b was centred at 0.45: nearer the target left column (0.25) but the actual
	// right one (centred at 0.6).
	assert.equal(a.tile, root.tiles[0]);
	assert.equal(b.tile, root.tiles[1]);
	assert.equal(c.tile, root.tiles[1]);
	assert.equal(errors.length, 1);
	assert.match(errors[0], /^opendeck-utilities layout: .*0\.5, 0\.5.*0\.2, 0\.8/);
});
