// @vitest-environment happy-dom
import { act, createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentCitationLink } from "@/components/agent/rendering/agent-citation-link";
import { MarkdownSurface } from "@/components/agent/rendering/markdown-surface";
import { resolveMobilePaperCitation } from "@/lib/mobile/paper-citation";
import { type Mounted, mountRoot } from "./helpers/mount";

vi.mock("react-i18next", () => import("./helpers/i18n"));
vi.mock("@/lib/workspace/actions", () => ({ openCitation: vi.fn() }));

let mounted: Mounted;

beforeEach(() => {
	mounted = mountRoot();
});

afterEach(async () => {
	await mounted.unmount();
});

describe("AgentCitationLink streaming guard", () => {
	it("passes the original encoded citation from the Markdown pipeline", async () => {
		const paper = { id: "shelf/100%20#v2", path: "papers/local" };
		const onOpenSource = vi.fn((source: string, encoded = source) =>
			resolveMobilePaperCitation(encoded, [paper]),
		);
		const href = "./agentero-paper/shelf%2F100%2520%23v2.pdf#page=4";
		await mounted.render(
			createElement(MarkdownSurface, { onOpenSource }, `[p. 4](<${href}>)`),
		);
		const link = mounted.container.querySelector<HTMLAnchorElement>("a");
		expect(link).not.toBeNull();
		await act(() => link?.click());
		expect(onOpenSource).toHaveReturnedWith({
			paper,
			kind: "pdf",
			fragment: { kind: "page", page: 4 },
		});
	});
	it("does not navigate an incomplete streaming citation", async () => {
		const onOpenSource = vi.fn();
		await mounted.render(
			createElement(
				AgentCitationLink,
				{ href: "streamdown:incomplete-link", onOpenSource },
				"Reference",
			),
		);
		const link = mounted.container.querySelector<HTMLAnchorElement>("a");
		expect(link?.hasAttribute("href")).toBe(false);
		await act(() => link?.click());
		expect(onOpenSource).not.toHaveBeenCalled();
	});
});
