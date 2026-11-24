import {
	type ApiResult,
	type ConnectionRef,
	commands,
	type HostRef,
	type OperationOutcome,
	type OperationRequest,
	type SessionRef,
	type WorkspaceRef,
} from "@/lib/core/bindings";
import { callApiResult, callResult, type TypedResult } from "@/lib/core/ipc";

async function mutationResult(
	call: () => Promise<TypedResult<ApiResult<OperationOutcome>>>,
): Promise<OperationOutcome> {
	const response = await callResult(call);
	// The service converts all post-dispatch errors to Unknown. An ApiResult error is a known preflight refusal.
	if (!response.ok)
		return {
			status: "rejected",
			code: response.error?.code ?? "remote_agent_rejected",
			message:
				response.error?.message ??
				"Remote Agent preflight rejected the operation",
		};
	if (!response.data) throw new Error("Missing Remote Agent operation receipt");
	return response.data;
}

// Every operation uses the generated Rust contract. Provider wire formats stay in Rust.
export const remoteAgent = {
	catalog: () => callApiResult(() => commands.remoteAgentCatalog()),
	oauthProviders: (server: string) =>
		callApiResult(() => commands.remoteAgentOauthProviders(server)),
	oauthStart: (server: string, providerId: string) =>
		callApiResult(() => commands.remoteAgentOauthStart(server, providerId)),
	oauthPoll: (qrId: string) =>
		callApiResult(() => commands.remoteAgentOauthPoll(qrId)),
	oauthCancel: (qrId: string) =>
		callApiResult(() => commands.remoteAgentOauthCancel(qrId)),
	logout: () => callApiResult(() => commands.remoteAgentLogout()),
	hosts: (connection: ConnectionRef) =>
		callApiResult(() => commands.remoteAgentHosts(connection)),
	identity: (host: HostRef) =>
		callApiResult(() => commands.remoteAgentHostIdentity(host)),
	trust: (host: HostRef, fingerprint: string) =>
		callApiResult(() => commands.remoteAgentTrustHost(host, fingerprint)),
	workspaces: (host: HostRef) =>
		callApiResult(() => commands.remoteAgentWorkspaces(host)),
	sessions: (workspace: WorkspaceRef) =>
		callApiResult(() => commands.remoteAgentSessions(workspace)),
	read: (session: SessionRef) =>
		callApiResult(() => commands.remoteAgentReadSession(session)),
	execute: (request: OperationRequest) =>
		mutationResult(() => commands.remoteAgentExecute(request)),
	sessionModels: (session: SessionRef) =>
		callApiResult(() => commands.remoteAgentSessionModels(session)),
	hostModels: (host: HostRef) =>
		callApiResult(() => commands.remoteAgentHostModels(host)),
	localOperations: (connection: ConnectionRef) =>
		callApiResult(() => commands.remoteAgentLocalOperations(connection)),
	receipt: (connection: ConnectionRef, operationId: string) =>
		callApiResult(() => commands.remoteAgentReceipt(connection, operationId)),
	upload: (session: SessionRef, path: string, operationId: string) =>
		mutationResult(() =>
			commands.remoteAgentUpload(session, path, operationId),
		),
};
