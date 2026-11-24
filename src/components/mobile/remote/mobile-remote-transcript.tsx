import { ChevronDownIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
	Reasoning,
	ReasoningContent,
	ReasoningTrigger,
} from "@/components/agent/elements/reasoning";
import {
	Tool,
	ToolContent,
	ToolHeader,
	ToolInput,
	ToolOutput,
} from "@/components/agent/elements/tool";
import {
	Message,
	MessageContent,
	MessageResponse,
} from "@/components/agent/message";
import { UserMessageResponse } from "@/components/agent/rendering/user-message-response";
import {
	Collapsible,
	CollapsibleContent,
	CollapsibleTrigger,
} from "@/components/ui/collapsible";
import type { RemoteEvent } from "@/lib/core/bindings";
import { cn } from "@/lib/core/utils";
import {
	buildRemoteTranscript,
	type RemoteTranscriptPart,
	type RemoteTranscriptRow,
} from "@/lib/remote-agent/transcript";

export type MobileRemoteTranscriptProps = {
	events: RemoteEvent[];
	sessionRunning: boolean;
	renderInteraction: (event: RemoteEvent) => ReactNode;
};

/** Fold reasoning/tool activity into one "Chain of Thought" block, like desktop. */
function ProcessCollapsible({ children }: { children: ReactNode }) {
	const { t } = useTranslation("aiElements");
	const [open, setOpen] = useState(false);
	return (
		<Collapsible
			open={open}
			onOpenChange={setOpen}
			className="not-prose w-full"
		>
			<CollapsibleTrigger className="flex min-h-11 w-full items-center gap-2 px-3 py-2 text-left text-muted-foreground text-sm transition-colors hover:text-foreground">
				<span className="font-medium">{t("chainOfThought.title")}</span>
				<ChevronDownIcon
					className={cn(
						"size-4 shrink-0 transition-transform",
						open && "rotate-180",
					)}
				/>
			</CollapsibleTrigger>
			<CollapsibleContent className="p-2">
				<div className="flex flex-col gap-1 rounded-lg border bg-muted/20 p-2">
					{children}
				</div>
			</CollapsibleContent>
		</Collapsible>
	);
}

function ActivityPart({
	part,
	insideProcess,
	streaming,
}: {
	part: RemoteTranscriptPart;
	insideProcess: boolean;
	streaming: boolean;
}) {
	const { t } = useTranslation("agent");
	if (part.kind === "reasoning") {
		if (!part.text.trim()) return null;
		return (
			<Reasoning
				className={insideProcess ? "mb-0" : "mb-2"}
				isStreaming={streaming}
				defaultOpen={false}
			>
				<ReasoningTrigger
					className={insideProcess ? "px-2 py-1.5 text-xs" : undefined}
					swapIconOnHover={insideProcess}
				/>
				<ReasoningContent
					className={insideProcess ? "mt-2 px-2 pb-2" : undefined}
				>
					{part.text}
				</ReasoningContent>
			</Reasoning>
		);
	}
	const state =
		part.state === "failed"
			? "output-error"
			: part.state === "finished"
				? "output-available"
				: "input-available";
	return (
		<Tool
			className={
				insideProcess
					? "mb-0 rounded-none border-0 bg-transparent shadow-none"
					: undefined
			}
		>
			<ToolHeader
				className={insideProcess ? "px-2 py-1.5" : undefined}
				swapIconOnHover={insideProcess}
				type="dynamic-tool"
				toolName={part.name}
				state={state}
			/>
			<ToolContent
				className={insideProcess ? "border-t-0 px-2 py-2" : undefined}
			>
				{part.input !== undefined ? <ToolInput input={part.input} /> : null}
				<ToolOutput
					output={part.output}
					errorText={part.state === "failed" ? t("tool.failed") : undefined}
				/>
			</ToolContent>
		</Tool>
	);
}

function AssistantRow({
	row,
	isLast,
}: {
	row: Extract<RemoteTranscriptRow, { kind: "assistant" }>;
	isLast: boolean;
}) {
	const streaming = row.streaming && isLast;
	const parts = row.parts;
	const lastIndex = parts.length - 1;
	const renderPart = (
		part: RemoteTranscriptPart,
		index: number,
		inside: boolean,
	) => (
		<ActivityPart
			key={part.id}
			part={part}
			insideProcess={inside}
			streaming={streaming && index === lastIndex && part.kind === "reasoning"}
		/>
	);
	const groupParts = !streaming && parts.length > 0;
	return (
		<Message from="assistant" className="max-w-full">
			<MessageContent className="w-full gap-3 text-base leading-relaxed">
				{groupParts ? (
					<ProcessCollapsible>
						{parts.map((part, index) => renderPart(part, index, true))}
					</ProcessCollapsible>
				) : (
					parts.map((part, index) => renderPart(part, index, false))
				)}
				{row.text ? (
					<div className="min-w-0">
						<MessageResponse isAnimating={streaming}>
							{row.text}
						</MessageResponse>
					</div>
				) : null}
			</MessageContent>
		</Message>
	);
}

/**
 * Remote session transcript with the desktop chat layout: human bubbles on
 * the right, assistant turns with collapsible reasoning and tool activity on
 * the left. Pure derivation from the polled snapshot — no local event state.
 */
export function MobileRemoteTranscript({
	events,
	sessionRunning,
	renderInteraction,
}: MobileRemoteTranscriptProps) {
	const rows = buildRemoteTranscript(events, sessionRunning);
	return (
		<>
			{rows.map((row, index) => {
				if (row.kind === "user") {
					return (
						<Message key={row.id} from="user" className="max-w-[85%]">
							<MessageContent className="text-base leading-6">
								<UserMessageResponse>{row.text}</UserMessageResponse>
							</MessageContent>
						</Message>
					);
				}
				if (row.kind === "interaction") {
					return <div key={row.id}>{renderInteraction(row.event)}</div>;
				}
				return (
					<AssistantRow
						key={row.id}
						row={row}
						isLast={index === rows.length - 1}
					/>
				);
			})}
		</>
	);
}
