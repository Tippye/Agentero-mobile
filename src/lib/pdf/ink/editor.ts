import type { InkEdit } from "@/lib/pdf/ink-api";
import type { InkController } from "./controller";
import {
	type InkBounds,
	type InkPosition,
	type InkSample,
	type InkShape,
	inkBounds,
	inkHitStroke,
	inkLassoBounds,
	inkLassoIds,
	moveInk,
} from "./geometry";
import { recognizeInkShape } from "./shapes";

export type InkMode = "read" | "pen" | "highlighter" | "eraser" | "lasso";
/** The in-progress freehand stroke; its points array grows in place. */
export type InkLiveStroke = { page: number; stroke: InkShape };
export type InkEditorState = Readonly<{
	mode: InkMode;
	color: string;
	width: number;
	paletteRequest: number;
	selection: { page: number; ids: string[]; bounds: InkBounds | null } | null;
	draft: { page: number; stroke: InkShape; snapped?: boolean } | null;
	lasso: { page: number; points: InkPosition[] } | null;
	moving: { page: number; strokes: InkShape[] } | null;
	erasing: { page: number; ids: string[] } | null;
}>;
export type InkInputPage = {
	page: number;
	width: number;
	height: number;
	rotation: number;
	element: HTMLElement;
};
type Gesture = {
	page: InkInputPage;
	tool: Exclude<InkMode, "read">;
	origin: InkPosition;
	samples: InkSample[];
	stroke?: InkShape;
	/** Hold-to-shape preview; lifting commits it, moving again restores ink. */
	snapped?: InkShape;
	originals?: InkShape[];
};
export const INK_COLORS = [
	"#202124",
	"#c92a2a",
	"#1864ab",
	"#2b8a3e",
	"#f0b429",
] as const;

/**
 * Synthesize point metadata along a snapped path: mean observed pressure and
 * arc-length-proportional elapsed time keep the native point contract (u32,
 * nondecreasing) without inventing device samples.
 */
function snappedStroke(
	base: InkShape,
	samples: readonly InkSample[],
	polyline: readonly InkPosition[],
): InkShape {
	let pressureSum = 0,
		pressureCount = 0;
	for (const sample of samples)
		if (sample.pressure !== null && Number.isFinite(sample.pressure)) {
			pressureSum += sample.pressure;
			pressureCount++;
		}
	const pressure = pressureCount ? pressureSum / pressureCount : null;
	const duration = samples.at(-1)?.elapsedMs ?? 0;
	const cumulative = [0];
	let total = 0;
	for (let i = 1; i < polyline.length; i++) {
		total += Math.hypot(
			polyline[i].x - polyline[i - 1].x,
			polyline[i].y - polyline[i - 1].y,
		);
		cumulative.push(total);
	}
	return {
		...base,
		points: polyline.map((point, i) => ({
			x: point.x,
			y: point.y,
			pressure,
			tilt: null,
			elapsedMs: total > 0 ? Math.round((duration * cumulative[i]) / total) : 0,
		})),
	};
}

/** Tool state is independent of native revisions and the PDF annotation plugin. */
export class InkEditor {
	private state: InkEditorState = {
		mode: "read",
		color: INK_COLORS[0],
		width: 2,
		paletteRequest: 0,
		selection: null,
		draft: null,
		lasso: null,
		moving: null,
		erasing: null,
	};
	private listeners = new Set<() => void>();
	private liveListeners = new Set<(live: InkLiveStroke) => void>();
	private gesture: Gesture | null = null;
	private previousMode: InkMode = "pen";
	constructor(readonly controller: InkController) {}
	subscribe = (listener: () => void) => {
		this.listeners.add(listener);
		return () => {
			this.listeners.delete(listener);
		};
	};
	/**
	 * Incremental live-stroke channel. The canvas overlay paints new segments
	 * directly, so a writing frame never broadcasts editor state to every page
	 * layer and the toolbar.
	 */
	subscribeLive = (listener: (live: InkLiveStroke) => void) => {
		this.liveListeners.add(listener);
		return () => {
			this.liveListeners.delete(listener);
		};
	};
	/** The freehand stroke being drawn, or null while snapped / not drawing. */
	getLive = (): InkLiveStroke | null => {
		const gesture = this.gesture;
		if (!gesture?.stroke || gesture.snapped) return null;
		return { page: gesture.page.page, stroke: gesture.stroke };
	};
	getState = () => this.state;
	getSelectionBounds = (): InkBounds | null => {
		const selection = this.state.selection;
		if (!selection) return null;
		return (
			selection.bounds ??
			inkBounds(
				this.controller
					.getPage(selection.page)
					.strokes.filter((stroke) => selection.ids.includes(stroke.id)),
			)
		);
	};
	private update(patch: Partial<InkEditorState>) {
		this.state = { ...this.state, ...patch };
		for (const listener of this.listeners) listener();
	}
	setMode(mode: InkMode) {
		if (mode !== this.state.mode) this.previousMode = this.state.mode;
		this.cancel();
		this.update({ mode, selection: null });
	}
	applyStylusAction(
		action:
			| "toggleEraser"
			| "previousTool"
			| "nextColor"
			| "showColorPalette"
			| "select",
	) {
		this.cancel();
		if (action === "toggleEraser")
			this.setMode(this.state.mode === "eraser" ? this.previousMode : "eraser");
		else if (action === "previousTool") this.setMode(this.previousMode);
		else if (action === "nextColor")
			this.setColor(
				INK_COLORS[
					(INK_COLORS.indexOf(this.state.color as (typeof INK_COLORS)[number]) +
						1) %
						INK_COLORS.length
				],
			);
		else if (action === "select") this.setMode("lasso");
		else this.update({ paletteRequest: this.state.paletteRequest + 1 });
	}
	/** Selecting an existing stroke or a source-PDF region never writes a mark. */
	selectAt(page: InkInputPage, point: InkPosition) {
		this.cancel();
		const ids = this.controller
			.getPage(page.page)
			.strokes.filter((stroke) => inkHitStroke(stroke, point, 8))
			.map((stroke) => stroke.id);
		const left = Math.max(0, point.x - 24),
			top = Math.max(0, point.y - 24);
		this.update({
			mode: "lasso",
			selection: {
				page: page.page,
				ids,
				bounds: ids.length
					? null
					: {
							x: left,
							y: top,
							width: Math.min(page.width, point.x + 24) - left,
							height: Math.min(page.height, point.y + 24) - top,
						},
			},
		});
	}
	setColor(color: string) {
		this.update({ color });
	}
	setWidth(width: number) {
		this.update({ width });
	}
	begin(page: InkInputPage, sample: InkSample, eraser = false): boolean {
		const mode = eraser ? "eraser" : this.state.mode;
		const state = this.controller.getPage(page.page);
		if (
			mode === "read" ||
			!state.snapshot ||
			state.loading ||
			this.controller.getOverview().dirty ||
			this.gesture
		)
			return false;
		const gesture: Gesture = {
			page,
			tool: mode,
			origin: sample,
			samples: [sample],
		};
		this.gesture = gesture;
		this.controller.setDrawing(true);
		if (mode === "pen" || mode === "highlighter") {
			const hex = this.state.color.slice(1);
			gesture.stroke = {
				id: crypto.randomUUID(),
				tool: mode,
				color: [
					parseInt(hex.slice(0, 2), 16),
					parseInt(hex.slice(2, 4), 16),
					parseInt(hex.slice(4, 6), 16),
					mode === "highlighter" ? 85 : 255,
				],
				baseWidth:
					mode === "highlighter" ? this.state.width * 6 : this.state.width,
				// Shared with gesture.samples: appends grow the live preview in
				// place and finish() commits exactly the accumulated samples.
				points: gesture.samples,
			};
			this.update({
				draft: { page: page.page, stroke: gesture.stroke },
				selection: null,
			});
		} else if (mode === "lasso") {
			const selected =
				this.state.selection?.page === page.page
					? state.strokes.filter((stroke) =>
							this.state.selection?.ids.includes(stroke.id),
						)
					: [];
			const bounds = inkBounds(selected);
			if (
				bounds &&
				sample.x >= bounds.x &&
				sample.x <= bounds.x + bounds.width &&
				sample.y >= bounds.y &&
				sample.y <= bounds.y + bounds.height
			)
				gesture.originals = selected;
			else
				this.update({
					selection: null,
					lasso: { page: page.page, points: [sample] },
				});
		} else
			this.update({
				erasing: { page: page.page, ids: this.eraseAt(page.page, sample, []) },
				selection: null,
			});
		return true;
	}
	private eraseAt(
		page: number,
		point: InkPosition,
		ids: string[],
		previous: InkPosition = point,
	): string[] {
		return [
			...new Set([
				...ids,
				...this.controller
					.getPage(page)
					.strokes.filter((stroke) => inkHitStroke(stroke, point, 8, previous))
					.map((stroke) => stroke.id),
			]),
		];
	}
	append(samples: InkSample[]) {
		const gesture = this.gesture;
		if (!gesture || !samples.length) return;
		let previous = gesture.samples.at(-1) ?? gesture.origin;
		gesture.samples.push(...samples);
		if (gesture.stroke) {
			// The live canvas paints new segments incrementally; a per-frame
			// state broadcast would re-render every page layer and the toolbar.
			// While snapped, jitter keeps accumulating for a possible restore but
			// the preview stays the clean shape.
			if (!gesture.snapped) {
				const live: InkLiveStroke = {
					page: gesture.page.page,
					stroke: gesture.stroke,
				};
				for (const listener of this.liveListeners) listener(live);
			}
		} else if (gesture.tool === "eraser") {
			let ids = this.state.erasing?.ids ?? [];
			for (const point of samples) {
				ids = this.eraseAt(gesture.page.page, point, ids, previous);
				previous = point;
			}
			this.update({ erasing: { page: gesture.page.page, ids } });
		} else if (gesture.originals) {
			const last = samples.at(-1) as InkSample;
			this.update({
				moving: {
					page: gesture.page.page,
					strokes: moveInk(
						gesture.originals,
						{ x: last.x - gesture.origin.x, y: last.y - gesture.origin.y },
						gesture.page,
					),
				},
			});
		} else
			this.update({
				lasso: { page: gesture.page.page, points: gesture.samples },
			});
	}
	/**
	 * Hold-to-shape: recognize the accumulated samples and preview a clean
	 * shape. Returns false when the stroke is not a confident shape, keeping
	 * handwriting as ink.
	 */
	snapDraft(): boolean {
		const gesture = this.gesture;
		if (!gesture?.stroke) return false;
		if (gesture.snapped) return true;
		const polyline = recognizeInkShape(gesture.samples);
		if (!polyline) return false;
		gesture.snapped = snappedStroke(gesture.stroke, gesture.samples, polyline);
		this.update({
			draft: {
				page: gesture.page.page,
				stroke: gesture.snapped,
				snapped: true,
			},
		});
		return true;
	}
	/** Drawing resumed after a snap: restore the freehand preview. */
	unsnapDraft() {
		const gesture = this.gesture;
		if (!gesture?.snapped || !gesture.stroke) return;
		gesture.snapped = undefined;
		this.update({
			draft: { page: gesture.page.page, stroke: gesture.stroke },
		});
	}
	finish() {
		const gesture = this.gesture;
		if (!gesture) return;
		let operations: InkEdit[] = [];
		if (gesture.stroke)
			operations = [{ kind: "put", stroke: gesture.snapped ?? gesture.stroke }];
		else if (gesture.tool === "eraser")
			operations = (this.state.erasing?.ids ?? []).map((id) => ({
				kind: "delete",
				id,
			}));
		else if (gesture.originals) {
			const moved = this.state.moving?.strokes ?? [];
			operations = moved.map((stroke) => ({ kind: "put", stroke }));
			if (moved.length)
				this.update({
					selection: {
						page: gesture.page.page,
						ids: moved.map((stroke) => stroke.id),
						// Transformed ink follows current native geometry, including undo/sync.
						bounds: null,
					},
				});
		} else
			this.update({
				selection: {
					page: gesture.page.page,
					bounds: inkLassoBounds(gesture.samples),
					ids: inkLassoIds(
						this.controller.getPage(gesture.page.page).strokes,
						gesture.samples,
					),
				},
			});
		this.gesture = null;
		// Move directly from transient geometry to pending native edits; parent
		// navigation guards must never observe a clean gap at pointerup.
		if (operations.length) this.controller.edit(gesture.page.page, operations);
		this.controller.setDrawing(false);
		this.update({ draft: null, lasso: null, moving: null, erasing: null });
	}
	cancel() {
		if (!this.gesture) return;
		this.gesture = null;
		this.controller.setDrawing(false);
		this.update({ draft: null, lasso: null, moving: null, erasing: null });
	}
	deleteSelection() {
		const selection = this.state.selection;
		if (!selection) return;
		if (
			this.controller.edit(
				selection.page,
				selection.ids.map((id) => ({ kind: "delete", id })),
			)
		)
			this.update({ selection: null });
	}
	copySelection(pageSize: { width: number; height: number }) {
		const selection = this.state.selection;
		if (!selection) return;
		const strokes = this.controller
			.getPage(selection.page)
			.strokes.filter((stroke) => selection.ids.includes(stroke.id));
		const copies = moveInk(strokes, { x: 12, y: 12 }, pageSize).map(
			(stroke) => ({
				...stroke,
				id: crypto.randomUUID(),
				createdAtMs: null,
				updatedAtMs: null,
			}),
		);
		if (
			this.controller.edit(
				selection.page,
				copies.map((stroke) => ({ kind: "put", stroke })),
			)
		)
			this.update({
				selection: {
					page: selection.page,
					ids: copies.map((stroke) => stroke.id),
					bounds: null,
				},
			});
	}
}
