export type MobileWorkspaceWidth = "narrow" | "medium" | "wide";
export type MobileWorkspacePreset = "read" | "compare" | "discuss";
export type MobileWorkspaceLayout = {
	library: boolean;
	reader: boolean;
	agent: boolean;
	compare: boolean;
};

export function mobileWorkspaceWidth(width: number): MobileWorkspaceWidth {
	return width < 600 ? "narrow" : width < 840 ? "medium" : "wide";
}

/** A preset changes visibility/sizes, never the ancestry of document surfaces. */
export function mobileWorkspaceLayout({
	width,
	preset,
	tab,
	hasPaper,
	showCitationReader = false,
}: {
	width: MobileWorkspaceWidth;
	preset: MobileWorkspacePreset;
	tab: "library" | "agent" | "plaza";
	hasPaper: boolean;
	showCitationReader?: boolean;
}): MobileWorkspaceLayout {
	const library = tab === "library";
	return {
		library: library && (!hasPaper || width === "wide"),
		reader:
			library &&
			hasPaper &&
			(width !== "narrow" || preset !== "discuss" || showCitationReader),
		agent: tab === "agent" || (library && hasPaper && preset === "discuss"),
		compare: library && hasPaper && width !== "narrow" && preset === "compare",
	};
}

export function canLeaveMobilePaper(
	notesDirty: boolean,
	inkDirty: boolean,
): boolean {
	return !notesDirty && !inkDirty;
}
