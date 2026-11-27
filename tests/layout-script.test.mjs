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
	{ id: "halves", widths: [0.5, 0.5] },
	{ id: "thirds", widths: [1 / 3, 1 / 3, 1 / 3] },
	{ id: "wide", widths: [0.25, 0.5, 0.25] },
];

function config(mode = "cycle", target = "wide") {
	return { mode, target, layouts: LAYOUTS, tolerance: 0.08 };
}

/**
 * A tile that behaves like KWin 6.8's CustomTile as far as the script uses it
 * (checked live on 2026-11-27): `split(1)` on a column in a horizontal row adds
 * a sibling after it and halves it; `split` on a leaf root makes two children;
 * `remove()` gives the space and the windows to the left neighbour (or the right
 * one for the first column); `relativeGeometry` reads are live.
 */
class FakeTile {
	constructor(parent, geom) {
		this.parent = parent;
		this.tiles = [];
		this.windows = [];
		this.layoutDirection = HORIZONTAL;
		this.geom = { ...geom };
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
		this.geom = { x: r.x, y: r.y, width: r.width, height: r.height };
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
		for (const w of windowsUnder(this)) home.take(w);
		this.parent = null;
	}

	manage(window) {
		manageCalls.push(window.name);
		this.take(window);
		return true;
	}

	take(window) {
		if (window.tile) window.tile.windows = window.tile.windows.filter((w) => w !== window);
		this.windows.push(window);
		window.tile = this;
	}
}

let manageCalls = [];

function windowsUnder(tile) {
	return [...tile.windows, ...tile.tiles.flatMap(windowsUnder)];
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
	const window = { name, tile: null };
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

test("cycle: halves goes to thirds, thirds to wide, wide wraps to halves", () => {
	assert.equal(script.pickTarget(config(), [0.5, 0.5]).id, "thirds");
	assert.equal(script.pickTarget(config(), [1 / 3, 1 / 3, 1 / 3]).id, "wide");
	assert.equal(script.pickTarget(config(), [0.25, 0.5, 0.25]).id, "halves");
});

test("cycle: widths a hair off an exact layout still count as that layout", () => {
	assert.equal(script.pickTarget(config(), [0.3335, 0.333, 0.3335]).id, "wide");
});

test("cycle: drifted columns are restored to the layout they drifted from", () => {
	assert.equal(script.pickTarget(config(), [0.55, 0.45]).id, "halves");
	assert.equal(script.pickTarget(config(), [0.3, 0.38, 0.32]).id, "thirds");
	assert.equal(script.pickTarget(config(), [0.28, 0.45, 0.27]).id, "wide");
});

test("cycle: with two layouts of the same count, the closer one is restored", () => {
	// At 0.08 thirds and wide cannot both match, so widen the tolerance to reach both.
	const loose = { ...config(), tolerance: 0.2 };
	assert.equal(script.pickTarget(loose, [0.27, 0.45, 0.28]).id, "wide");
	assert.equal(script.pickTarget(loose, [0.3, 0.4, 0.3]).id, "thirds");
});

test("cycle: an unknown layout goes to the first", () => {
	assert.equal(script.pickTarget(config(), [0.25, 0.25, 0.25, 0.25]).id, "halves");
	assert.equal(script.pickTarget(config(), [0.8, 0.2]).id, "halves");
	assert.equal(script.pickTarget(config(), []).id, "halves");
	assert.equal(script.pickTarget(config(), [1]).id, "halves");
});

test("set: the target is returned whatever the current layout", () => {
	for (const current of [[0.5, 0.5], [0.25, 0.5, 0.25], [0.8, 0.2], []]) {
		assert.equal(script.pickTarget(config("set", "wide"), current).id, "wide");
		assert.equal(script.pickTarget(config("set", "thirds"), current).id, "thirds");
	}
});

test("nearestColumn picks the column whose centre is nearest, ties go left", () => {
	assert.equal(script.nearestColumn(0.1, [0.5, 0.5]), 0);
	assert.equal(script.nearestColumn(0.9, [0.5, 0.5]), 1);
	assert.equal(script.nearestColumn(0.5, [0.5, 0.5]), 0);
	assert.equal(script.nearestColumn(0.5, [1 / 3, 1 / 3, 1 / 3]), 1);
	assert.equal(script.nearestColumn(5 / 6, [1 / 3, 1 / 3, 1 / 3]), 2);
	assert.equal(script.nearestColumn(0.125, [0.25, 0.5, 0.25]), 0);
});

test("apply: 3 to 2 columns keeps left left and right right; the middle goes left", () => {
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

test("apply: 25/50/25 to halves sends the middle window left", () => {
	const root = row([0.25, 0.5, 0.25]);
	const middle = windowIn("middle", root.tiles[1]);
	const right = windowIn("right", root.tiles[2]);
	script.apply(root, [0.5, 0.5]);
	assertColumns(root, [0.5, 0.5]);
	assert.equal(middle.tile, root.tiles[0]);
	assert.equal(right.tile, root.tiles[1]);
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

test("main: a cycle press moves halves to thirds", () => {
	const root = row([0.5, 0.5]);
	const errors = run(config(), root);
	assert.deepEqual(errors, []);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: drift is restored first, then the next press cycles on", () => {
	const root = row([0.28, 0.45, 0.27]);
	run(config(), root);
	assertColumns(root, [0.25, 0.5, 0.25]);
	run(config(), root);
	assertColumns(root, [0.5, 0.5]);
	run(config(), root);
	assertColumns(root, [1 / 3, 1 / 3, 1 / 3]);
});

test("main: an unknown layout goes to the first", () => {
	const four = row([0.25, 0.25, 0.25, 0.25]);
	run(config(), four);
	assertColumns(four, [0.5, 0.5]);
	const lopsided = row([0.8, 0.2]);
	run(config(), lopsided);
	assertColumns(lopsided, [0.5, 0.5]);
});

test("main: a root that is not a flat row counts as unknown and is rebuilt", () => {
	const root = new FakeTile(null, { x: 0, y: 0, width: 1, height: 1 });
	root.layoutDirection = VERTICAL;
	root.tiles = [
		new FakeTile(root, { x: 0, y: 0, width: 1, height: 0.5 }),
		new FakeTile(root, { x: 0, y: 0.5, width: 1, height: 0.5 }),
	];
	run(config(), root);
	assertColumns(root, [0.5, 0.5]);
});

test("main: set mode applies the target whatever the current layout", () => {
	for (const start of [[0.5, 0.5], [0.25, 0.5, 0.25], [0.8, 0.2], [0.25, 0.25, 0.25, 0.25], []]) {
		const root = row(start);
		run(config("set", "wide"), root);
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
