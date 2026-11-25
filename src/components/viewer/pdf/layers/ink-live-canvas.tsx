import { memo, useEffect, useRef } from "react";
import type { InkEditor } from "@/lib/pdf/ink/editor";
import type { InkPosition, InkSample, InkShape } from "@/lib/pdf/ink/geometry";

/** Mirrors inkOutline's pen thinning so live width matches the committed look. */
const PEN_THINNING = 0.65;

function liveWidth(stroke: InkShape, point: InkSample): number {
	const thinning = stroke.tool === "highlighter" ? 0 : PEN_THINNING;
	const pressure = point.pressure ?? 0.5;
	// perfect-freehand radius = size * (0.5 - thinning * (0.5 - pressure));
	// the live segment width is the matching diameter.
	return Math.max(
		0.1,
		stroke.baseWidth * (1 - thinning + 2 * thinning * pressure),
	);
}

function midpoint(a: InkPosition, b: InkPosition): InkPosition {
	return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/**
 * Incremental live-stroke overlay (the Xournal++ / Excalidraw pattern): new
 * samples are painted as pressure-wide quadratic segments onto an opaque mask,
 * then blitted once with the tool's alpha. No React state and no full-stroke
 * recompute — per-frame cost is O(new samples), and the mask keeps highlighter
 * joints from double-darkening. On pointerup the committed perfect-freehand
 * outline takes over in the SVG layer.
 */
export const InkLiveCanvas = memo(function InkLiveCanvas({
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
	const ref = useRef<HTMLCanvasElement>(null);
	useEffect(() => {
		const canvas = ref.current;
		const live = editor.getLive();
		if (!canvas || !live || live.page !== page) return;
		const stroke = live.stroke;
		// desynchronized: Chromium/WebView2 low-latency hint, bypassing the
		// compositor queue where the platform supports it.
		const ctx = canvas.getContext("2d", { desynchronized: true });
		const mask = document.createElement("canvas");
		const maskCtx = mask.getContext("2d");
		if (!ctx || !maskCtx) return;
		const [r, g, b, a] = stroke.color;
		canvas.style.mixBlendMode =
			stroke.tool === "highlighter" ? "multiply" : "normal";
		/** Points consumed into the curve state; the tail lags one sample. */
		let cursor = 0;
		let lastMid: InkPosition = { x: 0, y: 0 };
		let hasMid = false;
		const paint = () => {
			const points = stroke.points;
			if (cursor < 2) {
				if (!points.length) return;
				if (points.length === 1) {
					if (cursor === 0) {
						maskCtx.beginPath();
						maskCtx.arc(
							points[0].x,
							points[0].y,
							liveWidth(stroke, points[0]) / 2,
							0,
							Math.PI * 2,
						);
						maskCtx.fill();
						cursor = 1;
					}
				} else {
					if (!hasMid) {
						const m = midpoint(points[0], points[1]);
						maskCtx.lineWidth = liveWidth(stroke, points[0]);
						maskCtx.beginPath();
						maskCtx.moveTo(points[0].x, points[0].y);
						maskCtx.lineTo(m.x, m.y);
						maskCtx.stroke();
						lastMid = m;
						hasMid = true;
					}
					cursor = 2;
				}
			}
			for (; cursor < points.length; cursor++) {
				const control = points[cursor - 1];
				const end = midpoint(control, points[cursor]);
				maskCtx.lineWidth = liveWidth(stroke, control);
				maskCtx.beginPath();
				maskCtx.moveTo(lastMid.x, lastMid.y);
				maskCtx.quadraticCurveTo(control.x, control.y, end.x, end.y);
				maskCtx.stroke();
				lastMid = end;
			}
			ctx.setTransform(1, 0, 0, 1, 0, 0);
			ctx.clearRect(0, 0, canvas.width, canvas.height);
			ctx.globalAlpha = a / 255;
			ctx.drawImage(mask, 0, 0);
		};
		const resize = () => {
			const dpr = window.devicePixelRatio || 1;
			const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
			const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
			if (canvas.width === w && canvas.height === h && cursor > 0) return;
			canvas.width = w;
			canvas.height = h;
			mask.width = w;
			mask.height = h;
			// PDF points → device pixels; the frame keeps the page aspect.
			maskCtx.setTransform(w / width, 0, 0, h / height, 0, 0);
			maskCtx.lineCap = "round";
			maskCtx.lineJoin = "round";
			maskCtx.strokeStyle = `rgb(${r},${g},${b})`;
			maskCtx.fillStyle = `rgb(${r},${g},${b})`;
			cursor = 0;
			hasMid = false;
			paint();
		};
		resize();
		const unsubscribe = editor.subscribeLive((next) => {
			if (next.page === page && next.stroke.id === stroke.id) paint();
		});
		const observer =
			typeof ResizeObserver !== "undefined" ? new ResizeObserver(resize) : null;
		observer?.observe(canvas);
		return () => {
			unsubscribe();
			observer?.disconnect();
			mask.width = 0;
			mask.height = 0;
		};
	}, [editor, page, width, height]);
	return (
		<canvas
			ref={ref}
			aria-hidden="true"
			tabIndex={-1}
			className="pointer-events-none absolute inset-0 z-[9] h-full w-full"
		/>
	);
});
