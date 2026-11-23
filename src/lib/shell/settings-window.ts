/**
 * Native settings window control (single instance; `?window=settings` route).
 */

import { commands } from "@/lib/core/bindings";
import { notifyError } from "@/lib/core/notify";
import { isMobileApp, isTauri } from "@/lib/core/tauri";
import { openMobileSettings } from "@/lib/mobile/settings";
import { getVaultPath } from "@/lib/vault/store";
import { setSettingsOpenState, uiStore } from "./ui-store";

/** Map desktop settings sections onto the mobile settings page. */
function mobileSettingsSection(section?: string) {
	return section === "agent" ||
		section === "translate" ||
		section === "layout" ||
		section === "sync" ||
		section === "about"
		? section
		: ("appearance" as const);
}

export function openSettingsWindow(section?: string): void {
	if (isMobileApp()) {
		openMobileSettings(mobileSettingsSection(section));
		return;
	}
	if (!isTauri()) return;
	void (async () => {
		try {
			const res = await commands.settingsWindowOpen(
				section ?? null,
				getVaultPath(),
			);
			if (res.status === "error") {
				notifyError(res.error);
				return;
			}
			setSettingsOpenState(true);
		} catch (e) {
			notifyError(String(e));
		}
	})();
}

export function closeSettingsWindow(): void {
	if (isMobileApp() || !isTauri()) return;
	void (async () => {
		try {
			const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
			const win = await WebviewWindow.getByLabel("settings");
			await win?.close();
		} catch {
			// ignore
		}
	})();
}

export function toggleSettingsWindow(): void {
	if (uiStore.getState().settingsOpen) closeSettingsWindow();
	else openSettingsWindow();
}
