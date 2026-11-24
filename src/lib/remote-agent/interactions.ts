export const REMOTE_INTERACTION_PREFIX = "dsh-remote-request:";

export type RemoteElicitationResponse = {
	requestId: string;
	action: "accept" | "decline" | "cancel";
	content?: Record<string, string | string[]>;
};

type RemoteInteraction = {
	permission?: (optionId: string | null) => Promise<void>;
	elicitation?: (response: RemoteElicitationResponse) => Promise<void>;
};

export const remoteInteractions = new Map<string, RemoteInteraction>();

export function isRemoteInteractionId(requestId: string): boolean {
	return requestId.startsWith(REMOTE_INTERACTION_PREFIX);
}
