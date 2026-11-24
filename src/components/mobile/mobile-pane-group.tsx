import { Fragment, type ReactNode, useEffect, useRef, useState } from "react";
import type { GroupImperativeHandle, Layout } from "react-resizable-panels";
import {
	ResizableGroup,
	ResizableHandle,
	ResizablePanel,
} from "@/components/ui/resizable";

export type MobileWorkspacePane = {
	id: string;
	visible: boolean;
	content: ReactNode;
	/** Relative weight within the visible group. */
	weight: number;
	separatorLabel?: string;
};

/** Panels and their children always stay in the same keyed slots, even at 0%.
 * Only the public group's layout API changes between presets. */
export function MobilePaneGroup({
	id,
	layoutKey,
	panes,
	orientation = "horizontal",
}: {
	id: string;
	layoutKey: string;
	panes: MobileWorkspacePane[];
	orientation?: "horizontal" | "vertical";
}) {
	const group = useRef<GroupImperativeHandle>(null);
	const layouts = useRef(new Map<string, Layout>());
	const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
	const total = panes.reduce(
		(sum, pane) => sum + (pane.visible ? pane.weight : 0),
		0,
	);
	const desired = Object.fromEntries(
		panes.map((pane) => [
			`${id}-${pane.id}`,
			pane.visible ? (pane.weight * 100) / total : 0,
		]),
	);
	const desiredRef = useRef(desired);
	desiredRef.current = desired;
	useEffect(() => {
		group.current?.setLayout(
			layouts.current.get(layoutKey) ?? desiredRef.current,
		);
	}, [layoutKey]);
	return (
		<ResizableGroup
			id={id}
			groupRef={group}
			defaultLayout={layouts.current.get(layoutKey) ?? desired}
			orientation={orientation}
			className="h-full min-h-0 min-w-0"
			resizeTargetMinimumSize={{ coarse: 44, fine: 12 }}
			disabled={panes.filter((pane) => pane.visible).length < 2}
			onLayoutChanged={(layout, meta) => {
				if (meta.isUserInteraction) layouts.current.set(layoutKey, layout);
			}}
		>
			{panes.map((pane, index) => {
				const separator = pane.visible && panes[index + 1]?.visible;
				const hidden = !pane.visible || collapsed[pane.id] === true;
				return (
					<Fragment key={pane.id}>
						<ResizablePanel
							id={`${id}-${pane.id}`}
							minSize="18%"
							maxSize="100%"
							collapsible
							collapsedSize="0%"
							disabled={!pane.visible}
							onResize={({ asPercentage }) => {
								const next = asPercentage < 0.01;
								setCollapsed((previous) =>
									previous[pane.id] === next
										? previous
										: { ...previous, [pane.id]: next },
								);
							}}
						>
							<div
								data-workspace-pane={pane.id}
								hidden={hidden}
								inert={hidden}
								style={{ display: hidden ? "none" : undefined }}
								className="h-full min-h-0 min-w-0 overflow-hidden"
							>
								{pane.content}
							</div>
						</ResizablePanel>
						{index < panes.length - 1 ? (
							<ResizableHandle
								id={`${id}-${pane.id}-separator`}
								aria-label={pane.separatorLabel}
								disabled={!separator}
								hidden={!separator}
								style={{ display: separator ? undefined : "none" }}
								className={
									orientation === "vertical"
										? "h-px w-full after:inset-x-0 after:top-1/2 after:bottom-auto after:h-3 after:w-full after:translate-x-0 after:-translate-y-1/2 data-[separator=active]:bg-primary/50"
										: "after:w-3 data-[separator=active]:bg-primary/50"
								}
							/>
						) : null}
					</Fragment>
				);
			})}
		</ResizableGroup>
	);
}
