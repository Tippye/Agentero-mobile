import { ComposerAttachImageButton } from "@/components/agent/composer/composer-attachments";
import {
	Context,
	ContextContent,
	ContextContentHeader,
	ContextTrigger,
} from "@/components/agent/elements/context";

/** Renders inside `PromptInputTools`; the attach-image button needs the PromptInput attachment context. */
export function ComposerToolbar({
	compact = false,
	switching,
	activeUsage,
	showAttachments = true,
	allowFiles = false,
}: {
	compact?: boolean;
	switching: boolean;
	showAttachments?: boolean;
	allowFiles?: boolean;
	activeUsage: { used: number; size: number } | null;
}) {
	return (
		<>
			{!compact && activeUsage && activeUsage.size > 0 ? (
				<Context usedTokens={activeUsage.used} maxTokens={activeUsage.size}>
					<ContextTrigger className="h-7 gap-1 px-1.5 text-xs" />
					<ContextContent>
						<ContextContentHeader />
					</ContextContent>
				</Context>
			) : null}
			{!compact && showAttachments && (
				<ComposerAttachImageButton
					disabled={switching}
					allowFiles={allowFiles}
				/>
			)}
		</>
	);
}
