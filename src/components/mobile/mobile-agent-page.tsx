import type { TFunction } from "i18next";
import { ChevronDown, LoaderCircle, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
	PromptInput,
	PromptInputBody,
	PromptInputFooter,
	PromptInputSubmit,
	PromptInputTextarea,
} from "@/components/agent/composer/prompt-input";
import {
	Conversation,
	ConversationContent,
	ConversationEmptyState,
	ConversationScrollButton,
} from "@/components/agent/conversation";
import { Suggestion } from "@/components/agent/elements/suggestion";
import {
	Message,
	MessageContent,
	MessageResponse,
} from "@/components/agent/message";
import { useMobileAgentChat } from "@/components/mobile/hooks/use-mobile-agent-chat";
import type {
	AcpDeleteSessionResult,
	AcpListSessionsResult,
	AgentLine,
	AgentPermissionRequest,
	AgentStatusEvent,
	AgentTurnPhase,
} from "@/components/mobile/types";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuRadioGroup,
	DropdownMenuRadioItem,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
	MessageScrollerContent,
	MessageScrollerItem,
} from "@/components/ui/message-scroller";
import { Shimmer } from "@/components/ui/shimmer";
import type { AgentModelsEvent, WarmResult } from "@/lib/agent/api";
import { SUGGESTION_KEYS, SUGGESTION_WORKFLOW } from "@/lib/agent/chat-state";
import { displayHistoryTitle } from "@/lib/agent/prompt-display";
import { bridgeRpc } from "@/lib/bridge/client";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { cn } from "@/lib/core/utils";

const phaseLabelKey: Record<
	AgentTurnPhase,
	"agent.starting" | "agent.waitingModel" | "agent.reconnecting"
> = {
	starting: "agent.starting",
	"waiting-model": "agent.waitingModel",
	reconnecting: "agent.reconnecting",
};

/** Backend loading phase → status text; null falls back to the thinking label. */
function phaseStatusLabel(
	phase: AgentStatusEvent | null,
	t: TFunction<"mobile", undefined>,
): string | null {
	if (!phase) return null;
	const base = t(phaseLabelKey[phase.phase]);
	const detail = phase.detail?.trim();
	return detail ? `${base} ${detail}` : base;
}

export function MobileAgentPage({
	active = true,
	selectedAgentId,
	paperPath,
	sessionId,
	onSessionId,
	historyOpen,
	onHistoryOpenChange,
}: {
	/** Hidden mounted readers retain their session, but must not own global dialogs. */
	active?: boolean;
	selectedAgentId: string | null;
	paperPath?: string;
	sessionId: string | null;
	onSessionId: (sessionId: string | null) => void;
	historyOpen: boolean;
	onHistoryOpenChange: (open: boolean) => void;
}) {
	const { t } = useTranslation(["mobile", "agent", "common"]);
	const [modelId, setModelId] = useState<string>();
	const {
		lines,
		sending,
		restoring,
		permission,
		phase,
		restore,
		send,
		clearSession,
		respondToPermission,
	} = useMobileAgentChat({
		agentId: selectedAgentId,
		sessionId,
		onSessionId,
	});
	const streamingAssistant = [...lines]
		.reverse()
		.find((line) => line.role === "assistant" && line.streaming);
	const pendingLabel = phaseStatusLabel(phase, t);
	const composerDisabled = sending || restoring;
	const suggestions = paperPath
		? SUGGESTION_KEYS
		: SUGGESTION_KEYS.filter(
				(key) => key === "askLibrary" || key === "draftRelatedWork",
			);

	return (
		<>
			<section className="flex h-full min-h-0 flex-col">
				<Conversation className="min-h-0 flex-1">
					<ConversationContent className="mx-auto w-full max-w-3xl gap-3 px-4 py-3 md:px-6">
						<MessageScrollerContent className="gap-3">
							{restoring ? (
								<MessageScrollerItem messageId="restoring">
									<p className="mb-3 flex items-center gap-2 text-muted-foreground text-sm">
										<LoaderCircle className="size-4 animate-spin" />
										{t("agent.restoring")}
									</p>
								</MessageScrollerItem>
							) : null}
							{lines.length === 0 && !restoring ? (
								<MessageScrollerItem messageId="empty">
									<ConversationEmptyState className="min-h-0 flex-1 px-0 py-5">
										<h3 className="font-medium text-sm">{t("agent.empty")}</h3>
										<div className="flex w-full max-w-sm flex-col gap-1.5">
											{suggestions.map((key) => (
												<Suggestion
													key={key}
													suggestion={t(`agent:suggestions.${key}`)}
													className="h-auto min-h-11 w-full justify-start whitespace-normal rounded-lg px-3 py-2 text-left"
													disabled={composerDisabled}
													onClick={(value) =>
														void send(value, {
															modelId,
															workflow: SUGGESTION_WORKFLOW[key],
															target:
																key === "summarizePaper" || key === "listClaims"
																	? paperPath
																	: undefined,
														})
													}
												/>
											))}
										</div>
									</ConversationEmptyState>
								</MessageScrollerItem>
							) : (
								<>
									{lines.map((line) => (
										<MessageScrollerItem key={line.id} messageId={line.id}>
											<MobileChatLine
												line={line}
												label={
													line === streamingAssistant ? pendingLabel : null
												}
											/>
										</MessageScrollerItem>
									))}
									{sending && !streamingAssistant ? (
										<MessageScrollerItem messageId="pending">
											<Message from="assistant" className="max-w-full">
												<MessageContent className="text-sm leading-relaxed">
													<Shimmer className="text-sm">
														{pendingLabel ?? t("agent.thinking")}
													</Shimmer>
												</MessageContent>
											</Message>
										</MessageScrollerItem>
									) : null}
								</>
							)}
						</MessageScrollerContent>
					</ConversationContent>
					<ConversationScrollButton className="bottom-3 size-8 shadow-raised" />
				</Conversation>
				<PromptInput
					className="mx-auto max-w-3xl shrink-0 px-3 pt-1 pb-2 md:px-6"
					inputGroupClassName="rounded-xl bg-background shadow-raised"
					onSubmit={({ text: value }) => void send(value, { modelId })}
				>
					<PromptInputBody>
						<PromptInputTextarea
							aria-label={t("agent.placeholder")}
							placeholder={t("agent.placeholder")}
							disabled={composerDisabled}
							rows={1}
							className="min-h-10 max-h-32 resize-none px-3 pt-2.5 pb-1 text-base"
						/>
					</PromptInputBody>
					<PromptInputFooter className="px-1.5 py-0">
						<MobileBridgeModelMenu
							active={active}
							agentId={selectedAgentId}
							disabled={composerDisabled}
							modelId={modelId}
							onModelChange={setModelId}
						/>
						<PromptInputSubmit
							status={sending ? "submitted" : "ready"}
							disabled={composerDisabled}
							className="size-11 shrink-0 touch-manipulation"
						/>
					</PromptInputFooter>
				</PromptInput>
			</section>
			<MobilePermissionDialog
				active={active}
				permission={permission}
				onRespond={respondToPermission}
			/>
			<MobileAgentHistoryDialog
				active={active}
				open={historyOpen}
				agentId={selectedAgentId}
				currentSessionId={sessionId}
				onClose={() => onHistoryOpenChange(false)}
				onPick={(target) => {
					onHistoryOpenChange(false);
					void restore(target);
				}}
				onCurrentDeleted={clearSession}
			/>
		</>
	);
}

function MobileBridgeModelMenu({
	active,
	agentId,
	disabled,
	modelId,
	onModelChange,
}: {
	active: boolean;
	agentId: string | null;
	disabled: boolean;
	modelId: string | undefined;
	onModelChange: (modelId: string) => void;
}) {
	const { t } = useTranslation(["agent", "common"]);
	const [open, setOpen] = useState(false);
	const [catalog, setCatalog] = useState<AgentModelsEvent | null>();
	const [loading, setLoading] = useState(false);
	const [failed, setFailed] = useState(false);
	const loadInFlight = useRef(false);
	const currentId = modelId ?? catalog?.currentId;
	const selectedModel = catalog?.models.find((model) => model.id === currentId);

	useEffect(() => {
		if (!active) setOpen(false);
	}, [active]);

	const loadModels = async () => {
		if (loadInFlight.current || catalog !== undefined) return;
		loadInFlight.current = true;
		setLoading(true);
		setFailed(false);
		try {
			const result = await bridgeRpc<WarmResult>("agent_warm", {
				agentId: agentId ?? undefined,
			});
			if (!result.ok)
				throw new Error(result.error ?? t("common:somethingWrong"));
			setCatalog(result.models ?? null);
		} catch (error) {
			setFailed(true);
			notifyError(errorText(error) || t("common:somethingWrong"));
		} finally {
			loadInFlight.current = false;
			setLoading(false);
		}
	};

	return (
		<DropdownMenu
			open={active && open}
			onOpenChange={(next) => {
				setOpen(next);
				if (next) void loadModels();
			}}
		>
			<DropdownMenuTrigger asChild disabled={disabled}>
				<Button
					type="button"
					variant="ghost"
					size="sm"
					className="h-11 min-w-0 max-w-[calc(100%-3.25rem)] gap-1.5 rounded-lg px-2.5"
					title={t("models.selectTooltip")}
				>
					<span className="truncate">
						{selectedModel?.name ?? t("models.button")}
					</span>
					<ChevronDown className="size-4 shrink-0 text-muted-foreground" />
				</Button>
			</DropdownMenuTrigger>
			<DropdownMenuContent side="top" className="w-64 max-w-[calc(100vw-2rem)]">
				{loading ? (
					<DropdownMenuItem disabled className="min-h-11">
						{t("models.loading")}
					</DropdownMenuItem>
				) : failed ? (
					<DropdownMenuItem
						className="min-h-11"
						onSelect={(event) => {
							event.preventDefault();
							void loadModels();
						}}
					>
						{t("common:tryAgain")}
					</DropdownMenuItem>
				) : catalog?.models.length ? (
					<DropdownMenuRadioGroup
						value={currentId}
						onValueChange={onModelChange}
					>
						{catalog.models.map((model) => (
							<DropdownMenuRadioItem
								key={model.id}
								value={model.id}
								className="min-h-11"
							>
								<span className="min-w-0 wrap-break-word">{model.name}</span>
							</DropdownMenuRadioItem>
						))}
					</DropdownMenuRadioGroup>
				) : (
					<DropdownMenuItem disabled className="min-h-11">
						{t("models.emptyCatalog")}
					</DropdownMenuItem>
				)}
			</DropdownMenuContent>
		</DropdownMenu>
	);
}

function MobileChatLine({
	line,
	label = null,
}: {
	line: AgentLine;
	/** Phase-aware status text for the streaming placeholder. */
	label?: string | null;
}) {
	const { t } = useTranslation("mobile");
	const showThinking =
		line.role === "assistant" && !line.text.trim() && line.streaming;
	return (
		<Message
			from={line.role}
			className={line.role === "assistant" ? "max-w-full" : undefined}
		>
			<MessageContent
				className={cn(
					"text-sm leading-relaxed",
					line.role === "user" && "rounded-lg bg-muted px-3 py-2",
					line.role === "assistant" && "w-full max-w-full",
				)}
			>
				{showThinking ? (
					<Shimmer className="text-sm">{label ?? t("agent.thinking")}</Shimmer>
				) : (
					<MessageResponse
						isAnimating={line.streaming}
						enableOpenUiBlocks={line.role === "assistant"}
					>
						{line.text}
					</MessageResponse>
				)}
			</MessageContent>
		</Message>
	);
}

function MobileAgentHistoryDialog({
	active,
	open,
	agentId,
	currentSessionId,
	onClose,
	onPick,
	onCurrentDeleted,
}: {
	active: boolean;
	open: boolean;
	agentId: string | null;
	currentSessionId: string | null;
	onClose: () => void;
	onPick: (sessionId: string) => void;
	onCurrentDeleted: () => void;
}) {
	const { t } = useTranslation(["mobile", "common"]);
	const [loading, setLoading] = useState(false);
	const [result, setResult] = useState<AcpListSessionsResult | null>(null);
	const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
	const [deleting, setDeleting] = useState(false);
	const hiddenIds = useRef(new Set<string>());

	useEffect(() => {
		if (!active || !open) return;
		let current = true;
		setLoading(true);
		setResult(null);
		void bridgeRpc<AcpListSessionsResult>("agent_list_sessions", {
			agentId: agentId ?? undefined,
		})
			.then((next) => {
				if (!current) return;
				setResult({
					...next,
					sessions: next.sessions.filter(
						(session) => !hiddenIds.current.has(session.sessionId),
					),
				});
			})
			.catch(() => current && setResult({ sessions: [], supported: false }))
			.finally(() => current && setLoading(false));
		return () => {
			current = false;
		};
	}, [active, agentId, open]);

	const confirmDelete = async () => {
		if (!deleteTarget || deleting) return;
		setDeleting(true);
		try {
			const deleted = await bridgeRpc<AcpDeleteSessionResult>(
				"agent_delete_session",
				{
					agentId: agentId ?? undefined,
					sessionId: deleteTarget,
				},
			);
			const removedId = deleteTarget;
			hiddenIds.current.add(removedId);
			setResult((current) =>
				current
					? {
							...current,
							sessions: current.sessions.filter(
								(session) => session.sessionId !== removedId,
							),
						}
					: current,
			);
			setDeleteTarget(null);
			if (removedId === currentSessionId) onCurrentDeleted();
			if (deleted.agentError) {
				notifyError(t("agent.historyDeleteAgentFailed"), {
					description: deleted.agentError,
				});
			}
		} catch (error) {
			notifyError(errorText(error) || t("agent.historyDeleteFailed"));
		} finally {
			setDeleting(false);
		}
	};

	return (
		<Dialog
			open={active && open}
			onOpenChange={(next) => {
				if (active && !next) onClose();
			}}
		>
			{active ? (
				<DialogContent className="max-w-md rounded-lg">
					<DialogHeader>
						<DialogTitle>{t("agent.history")}</DialogTitle>
					</DialogHeader>
					{loading ? (
						<div className="grid place-items-center py-6">
							<LoaderCircle className="size-5 animate-spin text-muted-foreground" />
						</div>
					) : result && !result.supported ? (
						<p className="py-2 text-muted-foreground text-sm">
							{t("agent.historyUnsupported")}
						</p>
					) : result && result.sessions.length === 0 ? (
						<p className="py-2 text-muted-foreground text-sm">
							{t("agent.historyEmpty")}
						</p>
					) : (
						<ul className="agentero-scroll max-h-80 divide-y">
							{result?.sessions.map((session) => (
								<li key={session.sessionId}>
									<div className="flex items-stretch">
										<button
											type="button"
											className="flex min-w-0 flex-1 flex-col gap-0.5 px-1 py-3 text-left"
											onClick={() => onPick(session.sessionId)}
										>
											<span className="line-clamp-2 text-sm">
												{displayHistoryTitle(
													session.title ?? "",
													session.sessionId.slice(0, 8),
												)}
											</span>
											{session.updatedAt ? (
												<span className="text-muted-foreground text-xs">
													{session.updatedAt}
												</span>
											) : null}
										</button>
										<button
											type="button"
											className={cn(
												"my-auto mr-1 flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive/10 hover:text-destructive",
												deleteTarget === session.sessionId &&
													"bg-destructive/10 text-destructive",
											)}
											aria-label={t("agent.historyDelete")}
											aria-expanded={deleteTarget === session.sessionId}
											disabled={deleting}
											onClick={() =>
												setDeleteTarget((current) =>
													current === session.sessionId
														? null
														: session.sessionId,
												)
											}
										>
											<Trash2 className="size-4" aria-hidden />
										</button>
									</div>
									{deleteTarget === session.sessionId ? (
										<div className="flex justify-end px-1 pb-2">
											<div className="flex w-44 flex-col gap-2 rounded-lg bg-muted/40 p-2.5">
												<p className="font-medium text-sm leading-snug">
													{t("agent.historyDeleteTitle")}
												</p>
												<div className="flex justify-end gap-1">
													<Button
														type="button"
														size="xs"
														variant="ghost"
														disabled={deleting}
														onClick={() => setDeleteTarget(null)}
													>
														{t("common:cancel")}
													</Button>
													<Button
														type="button"
														size="xs"
														variant="destructive"
														disabled={deleting}
														onClick={() => void confirmDelete()}
													>
														{t("agent.historyDelete")}
													</Button>
												</div>
											</div>
										</div>
									) : null}
								</li>
							))}
						</ul>
					)}
				</DialogContent>
			) : null}
		</Dialog>
	);
}

function MobilePermissionDialog({
	active,
	permission,
	onRespond,
}: {
	active: boolean;
	permission: AgentPermissionRequest | null;
	onRespond: (optionId: string | null) => void;
}) {
	const { t } = useTranslation("agent");
	return (
		<Dialog
			open={active && permission !== null}
			onOpenChange={(open) => {
				if (active && !open) onRespond(null);
			}}
		>
			{active ? (
				<DialogContent showCloseButton={false} className="max-w-md rounded-lg">
					{permission ? (
						<>
							<DialogHeader>
								<DialogTitle>{t("permission.title")}</DialogTitle>
								<DialogDescription>{permission.title}</DialogDescription>
							</DialogHeader>
							{permission.host ? (
								<p className="text-xs text-muted-foreground">
									{t("permission.remoteHost", { host: permission.host })}
								</p>
							) : null}
							{permission.paths.length ? (
								<div className="space-y-1">
									{permission.paths.map((path) => (
										<code
											key={path}
											className="block truncate bg-muted px-2 py-1 text-xs"
											title={path}
										>
											{path}
										</code>
									))}
								</div>
							) : null}
							<DialogFooter className="sm:flex-col">
								{permission.options.map((option) => (
									<Button
										key={option.optionId}
										variant={
											option.kind.startsWith("allow") ? "default" : "outline"
										}
										onClick={() => onRespond(option.optionId)}
									>
										{option.name || option.kind}
									</Button>
								))}
								<Button variant="ghost" onClick={() => onRespond(null)}>
									{t("permission.deny")}
								</Button>
							</DialogFooter>
						</>
					) : null}
				</DialogContent>
			) : null}
		</Dialog>
	);
}
