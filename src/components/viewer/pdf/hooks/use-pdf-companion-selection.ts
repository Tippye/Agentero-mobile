import { type ComponentProps, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { addSelectionToChat } from "@/components/selection/add-selection-to-chat";
import type { SelectionMenu } from "@/components/viewer/pdf/cards/selection-menu";
import type { PdfHighlights } from "@/components/viewer/pdf/hooks/use-pdf-highlights";
import {
	type UsePdfTranslationSelectionOptions,
	usePdfTranslationSelection,
} from "@/components/viewer/pdf/hooks/use-pdf-translation-selection";
import type {
	ScreenPoint,
	SelectionCommentDraft,
	SelectionMenuState,
} from "@/components/viewer/pdf/types";
import { copyTextToClipboard } from "@/lib/core/clipboard";
import type { PdfAskAnchor } from "@/lib/pdf/ask/types";
import { visibleMenuPoint } from "@/lib/pdf/layout/visible-selection-rects";
import { translationPageFrames } from "./use-translation-selection-bands";

type Options = Omit<
	UsePdfTranslationSelectionOptions,
	"selectionMenu" | "setSelectionMenu" | "closeSelectionMenu"
> & {
	createHighlights: PdfHighlights["createHighlights"];
	canMark: boolean;
	zoom: number;
	onAsk: (
		anchor: PdfAskAnchor,
		translation: string | undefined,
		screen: ScreenPoint,
	) => void;
	onNote: (draft: SelectionCommentDraft) => void;
};

/** Translated DOM text shares one evidence menu in the reader and companion. */
export function usePdfTranslatedEvidence({
	createHighlights,
	canMark,
	zoom,
	onAsk,
	onNote,
	...options
}: Options) {
	const { enabled, hostRef, paperRelPath, paperAbsPath } = options;
	const { t } = useTranslation("viewer");
	const [selectionMenu, setSelectionMenu] = useState<SelectionMenuState | null>(
		null,
	);
	const { closeSelectionMenu } = usePdfTranslationSelection({
		...options,
		selectionMenu,
		setSelectionMenu,
	});
	const rePlaceSelectionMenu = useCallback(() => {
		const host = hostRef.current;
		if (!host) return;
		setSelectionMenu((menu) => {
			if (!menu?.visiblePages?.length) return menu;
			const frames = translationPageFrames(host);
			const screen = visibleMenuPoint(
				menu.visiblePages,
				menu.anchor.page - 1,
				(page) => frames.find((frame) => frame.pageIndex === page) ?? null,
				menu.cursorXFraction,
			);
			if (
				!screen ||
				(screen.x === menu.screen.x &&
					screen.top === menu.screen.top &&
					screen.bottom === menu.screen.bottom &&
					screen.cursorX === menu.screen.cursorX)
			)
				return menu;
			return { ...menu, screen: { ...screen, side: menu.screen.side } };
		});
	}, [hostRef]);
	useEffect(() => {
		if (zoom > 0) rePlaceSelectionMenu();
	}, [zoom, rePlaceSelectionMenu]);

	let menu: ComponentProps<typeof SelectionMenu> | null = null;
	if (enabled && selectionMenu) {
		const { anchor, copyText, pairedTranslation, pages, screen, visiblePages } =
			selectionMenu;
		const visibleRects = visiblePages?.find(
			(page) => page.pageIndex === anchor.page - 1,
		)?.rects;
		const markable = canMark && pages.length > 0;
		menu = {
			screen,
			selectionText: copyText ?? anchor.quote,
			showHighlight: markable,
			showTranslate: false,
			onCopy: () =>
				copyTextToClipboard(copyText ?? anchor.quote ?? "", {
					errorMessage: t("selection.copyFailed"),
				}),
			onAsk: () => {
				closeSelectionMenu();
				onAsk(
					visibleRects?.length
						? { ...anchor, translatedRects: visibleRects }
						: anchor,
					pairedTranslation,
					screen,
				);
			},
			onAddToChat: () => {
				if (!anchor.quote) return;
				closeSelectionMenu();
				addSelectionToChat({
					text: anchor.quote,
					sourcePath: paperRelPath ?? paperAbsPath ?? "PDF",
					origin: "pdf",
					page: anchor.page,
					rects: anchor.rects,
					paperAbsPath: paperAbsPath ?? undefined,
					context: pairedTranslation
						? { status: "unavailable", translation: pairedTranslation }
						: undefined,
				});
			},
			onHighlight: markable
				? (color, shape) => {
						createHighlights(
							pages,
							color,
							anchor.quote ?? "",
							visiblePages,
							shape,
						);
						closeSelectionMenu();
					}
				: undefined,
			onNote: markable
				? (shape) => {
						onNote({
							page: anchor.page,
							anchorY: visibleRects?.[0]?.y ?? anchor.rects[0]?.y ?? 0,
							quote: anchor.quote ?? "",
							shape,
							pages,
							visiblePages,
						});
						closeSelectionMenu();
					}
				: undefined,
		};
	}
	return { menu, rePlaceSelectionMenu };
}
