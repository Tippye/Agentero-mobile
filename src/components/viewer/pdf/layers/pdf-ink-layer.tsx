import { memo, useEffect, useMemo, useSyncExternalStore } from "react";
import { InkLiveCanvas } from "@/components/viewer/pdf/layers/ink-live-canvas";
import { useSettings } from "@/hooks/use-app-stores";
import type { InkEditor } from "@/lib/pdf/ink/editor";
import {
	DEFAULT_INK_FEEL,
	type InkFeelTuning,
	type InkShape,
	inkBounds,
	inkOutline,
	inkSvgPath,
} from "@/lib/pdf/ink/geometry";

const outlines = new WeakMap<InkShape, { key: string; d: string }>();
const InkPath = memo(function InkPath({
	stroke,
	complete = true,
	dim = false,
	tuning = DEFAULT_INK_FEEL,
}: {
	stroke: InkShape;
	complete?: boolean;
	dim?: boolean;
	tuning?: InkFeelTuning;
}) {
	const key = `${tuning.thinning}|${tuning.pressureFloor}|${tuning.streamline}|${tuning.smoothing}|${tuning.simulatePressure}`;
	const cached = complete ? outlines.get(stroke) : undefined;
	let d = cached?.key === key ? cached.d : undefined;
	if (d === undefined) {
		d = inkSvgPath(inkOutline(stroke, complete, tuning));
		if (complete) outlines.set(stroke, { key, d });
	}
	const [r, g, b, a] = stroke.color;
	return (
		<path
			d={d}
			fill={`rgb(${r},${g},${b})`}
			opacity={(a / 255) * (dim ? 0.25 : 1)}
			style={{
				mixBlendMode: stroke.tool === "highlighter" ? "multiply" : "normal",
			}}
		/>
	);
});

/** The existing UnrotatedPageFrame owns page rotation and CSS zoom. */
export const PdfInkLayer = memo(function PdfInkLayer({
	editor,
	page,
	width,
	height,
}: {
	editor: InkEditor;
	page: number;
	width: number;
	height: number;
}) {
	const state = useSyncExternalStore(editor.controller.subscribe, () =>
		editor.controller.getPage(page),
	);
	const tools = useSyncExternalStore(editor.subscribe, editor.getState);
	const thinning = useSettings((s) => s.pdfInkThinning);
	const pressureFloor = useSettings((s) => s.pdfInkPressureFloor);
	const streamline = useSettings((s) => s.pdfInkStreamline);
	const smoothing = useSettings((s) => s.pdfInkSmoothing);
	const simulatePressure = useSettings((s) => s.pdfInkSimulatePressure);
	const tuning = useMemo(
		() => ({
			thinning,
			pressureFloor,
			streamline,
			smoothing,
			simulatePressure,
		}),
		[thinning, pressureFloor, streamline, smoothing, simulatePressure],
	);
	useEffect(() => {
		void editor.controller.load(page);
	}, [editor, page]);
	const moving = tools.moving?.page === page ? tools.moving.strokes : null;
	const movedIds = new Set(moving?.map((stroke) => stroke.id));
	const erasedIds = new Set(
		tools.erasing?.page === page ? tools.erasing.ids : [],
	);
	const strokes = moving
		? [...state.strokes.filter((stroke) => !movedIds.has(stroke.id)), ...moving]
		: state.strokes;
	const selection =
		tools.selection?.page === page
			? moving
				? inkBounds(moving)
				: (tools.selection.bounds ??
					inkBounds(
						strokes.filter((stroke) =>
							tools.selection?.ids.includes(stroke.id),
						),
					))
			: null;
	const lasso = tools.lasso?.page === page ? tools.lasso.points : null;
	const draft = tools.draft?.page === page ? tools.draft : null;
	return (
		<>
			<svg
				data-pdf-ink-page={page}
				data-ink-ready={!!state.snapshot && !state.loading && !state.error}
				aria-hidden="true"
				className="pointer-events-none absolute inset-0 z-[9] overflow-hidden"
				width="100%"
				height="100%"
				viewBox={`0 0 ${width} ${height}`}
			>
				{strokes.map((stroke) => (
					<InkPath
						key={stroke.id}
						stroke={stroke}
						dim={erasedIds.has(stroke.id)}
						tuning={tuning}
					/>
				))}
				{draft?.snapped === true && (
					<InkPath stroke={draft.stroke} complete tuning={tuning} />
				)}
				{selection && (
					<rect
						{...selection}
						fill="none"
						stroke="var(--primary)"
						strokeWidth={1}
						strokeDasharray="4 3"
						vectorEffect="non-scaling-stroke"
					/>
				)}
				{lasso && (
					<path
						d={lasso
							.map(
								(point, index) => `${index ? "L" : "M"}${point.x},${point.y}`,
							)
							.join(" ")}
						fill="none"
						stroke="var(--primary)"
						strokeWidth={1.5}
						strokeDasharray="4 3"
						vectorEffect="non-scaling-stroke"
					/>
				)}
			</svg>
			{/* Freehand ink paints incrementally on canvas, outside the SVG; the
			    hold-to-shape preview stays an SVG path above. */}
			{draft && draft.snapped !== true && (
				<InkLiveCanvas
					key={draft.stroke.id}
					editor={editor}
					page={page}
					width={width}
					height={height}
					tuning={tuning}
				/>
			)}
		</>
	);
});
