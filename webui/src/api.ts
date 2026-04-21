
// Determine the API base URL:
// - Use VITE_API_BASE from environment if provided
// - Fallback to current origin (useful for production/docker)
function getApiBase(): string {
    const envApiBase = import.meta.env.VITE_API_BASE;
    if (envApiBase) {
        return envApiBase;
    }

    if (typeof window === 'undefined') {
        return 'http://127.0.0.1:8787';
    }

    const origin = window.location.origin;

    // In Vite dev server, if no env var is provided, we still need a fallback for local dev
    if (origin.includes('localhost') || origin.includes('127.0.0.1')) {
        return 'http://127.0.0.1:8787';
    }

    // For production browser access, use the current origin
    return origin;
}

export const API_BASE = getApiBase();

// Skills registry URL (CF Worker)
const REGISTRY_URL = import.meta.env.VITE_LINGGEN_CF_WORKER_URL || 'https://linggen-analytics.liangatbc.workers.dev';
const REGISTRY_API_KEY = import.meta.env.VITE_LINGGEN_CF_WORKER_API_KEY;

/** Indexing mode: "full" rebuilds everything, "incremental" only updates changed files */
export type IndexMode = 'full' | 'incremental';

export interface IndexSourceRequest {
    source_id: string;
    mode?: IndexMode;
}

export interface IndexSourceResponse {
    job_id: string;
    files_indexed: number;
    chunks_created: number;
}

/**
 * Index a source with optional mode.
 * @param sourceId The source ID to index
 * @param mode "incremental" (default) only indexes changed files, "full" rebuilds everything
 */
export async function indexSource(sourceId: string, mode: IndexMode = 'incremental'): Promise<IndexSourceResponse> {
    const response = await fetch(`${API_BASE}/api/index_source`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ source_id: sourceId, mode }),
    });

    if (!response.ok) {
        const text = await response.text();
        throw new Error(`Failed to index source: ${text} `);
    }

    return response.json();
}

// Resource Management
export type ResourceType = 'git' | 'local' | 'web' | 'uploads';

export interface SourceStats {
    chunk_count: number;
    file_count: number;
    total_size_bytes: number;
}

export interface Resource {
    id: string;
    name: string;
    resource_type: ResourceType;
    path: string;
    enabled: boolean;
    include_patterns: string[];
    exclude_patterns: string[];
    latest_job?: Job;
    stats?: SourceStats;
    last_upload_time?: string;
}

export interface AddResourceRequest {
    name: string;
    resource_type: ResourceType;
    path: string;
    include_patterns?: string[];
    exclude_patterns?: string[];
}

export interface AddResourceResponse {
    id: string;
    name: string;
    resource_type: ResourceType;
    path: string;
    enabled: boolean;
    include_patterns: string[];
    exclude_patterns: string[];
}

export interface ListResourcesResponse {
    resources: Resource[];
}

export interface RemoveResourceResponse {
    success: boolean;
    id: string;
}

export async function addResource(req: AddResourceRequest): Promise<AddResourceResponse> {
    const response = await fetch(`${API_BASE}/api/resources`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify(req),
    });

    if (!response.ok) {
        throw new Error(`Failed to add resource: ${response.statusText} `);
    }

    return response.json();
}

export async function listResources(): Promise<ListResourcesResponse> {
    const response = await fetch(`${API_BASE}/api/resources`);

    if (!response.ok) {
        throw new Error(`Failed to list resources: ${response.statusText} `);
    }

    return response.json();
}

export async function removeResource(id: string): Promise<RemoveResourceResponse> {
    const response = await fetch(`${API_BASE}/api/resources/remove`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ id }),
    });

    if (!response.ok) {
        throw new Error(`Failed to remove resource: ${response.statusText} `);
    }

    return response.json();
}

export interface RenameResourceResponse {
    success: boolean;
    id: string;
    name: string;
}

export async function renameResource(id: string, name: string): Promise<RenameResourceResponse> {
    const response = await fetch(`${API_BASE}/api/resources/rename`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ id, name }),
    });

    if (!response.ok) {
        throw new Error(`Failed to rename resource: ${response.statusText} `);
    }

    return response.json();
}

export interface UpdateResourcePatternsResponse {
    success: boolean;
    id: string;
    include_patterns: string[];
    exclude_patterns: string[];
}

export async function updateResourcePatterns(
    id: string,
    include_patterns: string[],
    exclude_patterns: string[]
): Promise<UpdateResourcePatternsResponse> {
    const response = await fetch(`${API_BASE}/api/resources/patterns`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ id, include_patterns, exclude_patterns }),
    });

    if (!response.ok) {
        throw new Error(`Failed to update patterns: ${response.statusText} `);
    }

    return response.json();
}

// File Upload for Uploads sources
export interface UploadFileResponse {
    success: boolean;
    source_id: string;
    filename: string;
    chunks_created: number;
}

export interface UploadProgressInfo {
    phase: string;
    progress: number;
    message: string;
    error?: string;
    result?: UploadFileResponse;
}

// Upload with streaming progress (shows extracting, chunking, embedding progress)
export async function uploadFileWithProgress(
    sourceId: string,
    file: File,
    onProgress?: (info: UploadProgressInfo) => void
): Promise<UploadFileResponse> {
    const formData = new FormData();
    formData.append('source_id', sourceId);
    formData.append('file', file);

    const response = await fetch(`${API_BASE}/api/upload/stream`, {
        method: 'POST',
        body: formData,
    });

    if (!response.ok) {
        throw new Error(`Upload failed: ${response.statusText} `);
    }

    const reader = response.body?.getReader();
    if (!reader) {
        throw new Error('No response body');
    }

    const decoder = new TextDecoder();
    let buffer = '';
    let result: UploadFileResponse | null = null;

    while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });

        // Parse SSE events (data: {...}\n\n)
        const lines = buffer.split('\n\n');
        buffer = lines.pop() || ''; // Keep incomplete data in buffer

        for (const line of lines) {
            if (line.startsWith('data: ')) {
                try {
                    const data = JSON.parse(line.slice(6)) as UploadProgressInfo;
                    if (onProgress) {
                        onProgress(data);
                    }
                    if (data.error) {
                        throw new Error(data.error);
                    }
                    if (data.result) {
                        result = data.result;
                    }
                } catch (e) {
                    if (e instanceof Error && e.message !== 'Unexpected end of JSON input') {
                        throw e;
                    }
                }
            }
        }
    }

    if (!result) {
        throw new Error('Upload completed but no result received');
    }

    return result;
}

// Simple upload without detailed progress (legacy)
export async function uploadFile(
    sourceId: string,
    file: File,
    onProgress?: (percent: number) => void
): Promise<UploadFileResponse> {
    const formData = new FormData();
    formData.append('source_id', sourceId);
    formData.append('file', file);

    return new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();

        xhr.upload.addEventListener('progress', (event) => {
            if (event.lengthComputable && onProgress) {
                const percent = Math.round((event.loaded / event.total) * 100);
                onProgress(percent);
            }
        });

        xhr.addEventListener('load', () => {
            if (xhr.status >= 200 && xhr.status < 300) {
                try {
                    const response = JSON.parse(xhr.responseText);
                    resolve(response);
                } catch {
                    reject(new Error('Failed to parse response'));
                }
            } else {
                try {
                    const errorData = JSON.parse(xhr.responseText);
                    reject(new Error(errorData.error || `Upload failed: ${xhr.statusText} `));
                } catch {
                    reject(new Error(`Upload failed: ${xhr.statusText} `));
                }
            }
        });

        xhr.addEventListener('error', () => {
            reject(new Error('Network error during upload'));
        });

        xhr.open('POST', `${API_BASE}/api/upload`);
        xhr.send(formData);
    });
}

// List uploaded files for a source
export interface FileInfo {
    filename: string;
    chunk_count: number;
}

export interface ListFilesResponse {
    source_id: string;
    files: FileInfo[];
}

export async function listUploadedFiles(sourceId: string): Promise<ListFilesResponse> {
    const response = await fetch(`${API_BASE}/api/upload/files`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ source_id: sourceId }),
    });

    if (!response.ok) {
        throw new Error(`Failed to list files: ${response.statusText} `);
    }

    return response.json();
}

// Delete an uploaded file
export interface DeleteFileResponse {
    success: boolean;
    source_id: string;
    filename: string;
    chunks_deleted: number;
}

export async function deleteUploadedFile(sourceId: string, filename: string): Promise<DeleteFileResponse> {
    const response = await fetch(`${API_BASE}/api/upload/delete`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ source_id: sourceId, filename }),
    });

    if (!response.ok) {
        throw new Error(`Failed to delete file: ${response.statusText} `);
    }

    return response.json();
}

// Design Notes API
export interface NoteContent {
    path: string;
    content: string;
    linked_node?: string;
}

export interface Note {
    name: string;
    path: string; // Relative to .linggen/notes
    modified_at?: string;
}

export interface ListNotesResponse {
    notes: Note[];
}

export async function listNotes(sourceId: string): Promise<Note[]> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/notes`);
    if (!response.ok) {
        if (response.status === 404) {
            return [];
        }
        throw new Error(`Failed to list notes: ${response.statusText}`);
    }
    const data: ListNotesResponse = await response.json();
    return data.notes;
}

export async function getNote(sourceId: string, notePath: string): Promise<NoteContent> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/notes/${encodeURIComponent(notePath)}`);
    if (!response.ok) {
        throw new Error(`Failed to get note: ${response.statusText}`);
    }
    return response.json();
}

export async function deleteNote(sourceId: string, notePath: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/notes/${encodeURIComponent(notePath)}`, {
        method: 'DELETE',
    });

    if (!response.ok) {
        throw new Error(`Failed to delete note: ${response.statusText}`);
    }
}

export interface SaveNoteRequest {
    content: string;
    linked_node?: string;
}

export async function saveNote(sourceId: string, notePath: string, content: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/notes/${encodeURIComponent(notePath)}`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ content }),
    });

    if (!response.ok) {
        throw new Error(`Failed to save note: ${response.statusText}`);
    }
}

// Source Memory Files API (markdown under source/.linggen/memory)
export interface MemoryFile {
    name: string;
    path: string; // Relative to .linggen/memory
    modified_at?: string;
}

export interface ListMemoryFilesResponse {
    files: MemoryFile[];
}

export interface MemoryFileContent {
    path: string;
    content: string;
}

export async function listMemoryFiles(sourceId: string): Promise<MemoryFile[]> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/memory`);
    if (!response.ok) {
        if (response.status === 404) {
            return [];
        }
        throw new Error(`Failed to list memory files: ${response.statusText}`);
    }
    const data: ListMemoryFilesResponse = await response.json();
    return data.files || [];
}

export async function getMemoryFile(sourceId: string, filePath: string): Promise<MemoryFileContent> {
    const response = await fetch(
        `${API_BASE}/api/sources/${sourceId}/memory/${encodeURIComponent(filePath)}`,
    );
    if (!response.ok) {
        throw new Error(`Failed to get memory file: ${response.statusText}`);
    }
    return response.json();
}

export async function saveMemoryFile(sourceId: string, filePath: string, content: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/memory/${encodeURIComponent(filePath)}`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ content }),
    });
    if (!response.ok) {
        throw new Error(`Failed to save memory file: ${response.statusText}`);
    }
}

export interface GraphStatusResponse {
    status: 'missing' | 'stale' | 'ready' | 'building' | 'error';
    node_count?: number;
    edge_count?: number;
    built_at?: string;
}



// Delete an uploaded file


// Jobs
export type JobStatus = 'Pending' | 'Running' | 'Completed' | 'Failed';

export interface Job {
    id: string;
    source_id: string;
    source_name: string;
    source_type: ResourceType;
    status: JobStatus;
    started_at: string;
    finished_at?: string;
    files_indexed?: number;
    chunks_created?: number;
    total_files?: number;
    total_size_bytes?: number;
    error?: string;
}

export interface ListJobsResponse {
    jobs: Job[];
}

export async function listJobs(): Promise<ListJobsResponse> {
    const response = await fetch(`${API_BASE}/api/jobs`);

    if (!response.ok) {
        throw new Error(`Failed to list jobs: ${response.statusText}`);
    }

    return response.json();
}

export interface CancelJobRequest {
    job_id: string;
}

export interface CancelJobResponse {
    success: boolean;
    job_id: string;
}

export async function cancelJob(jobId: string): Promise<CancelJobResponse> {
    const response = await fetch(`${API_BASE}/api/jobs/cancel`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ job_id: jobId }),
    });

    if (!response.ok) {
        throw new Error(`Failed to cancel job: ${response.statusText}`);
    }

    return response.json();
}

// Intent Classification
export interface IntentClassifyRequest {
    query: string;
}

export type IntentType =
    | 'fix_bug'
    | 'explain_code'
    | 'refactor_code'
    | 'write_test'
    | 'debug_error'
    | 'generate_doc'
    | 'analyze_performance'
    | 'ask_question'
    | { other: string };

export interface IntentClassifyResponse {
    intent: IntentType;
    confidence: number;
    entities: string[];
    needs_context: boolean;
}

export async function classifyIntent(query: string): Promise<IntentClassifyResponse> {
    const response = await fetch(`${API_BASE}/api/classify`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ query }),
    });

    if (!response.ok) {
        const text = await response.text();
        throw new Error(`Failed to classify intent: ${text}`);
    }

    return response.json();
}

// Prompt Enhancement
export type PromptStrategy = 'full_code' | 'reference_only' | 'architectural';

export interface EnhancePromptRequest {
    query: string;
    strategy?: PromptStrategy;
}

export interface EnhancedPromptResponse {
    original_query: string;
    enhanced_prompt: string;
    intent: IntentType;
    context_chunks: string[];
    context_metadata?: {
        source_id: string;
        document_id: string;
        file_path: string;
    }[];
    preferences_applied: boolean;
}

export async function enhancePrompt(query: string, strategy?: PromptStrategy): Promise<EnhancedPromptResponse> {
    const response = await fetch(`${API_BASE}/api/enhance`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ query, strategy }),
    });

    if (!response.ok) {
        const text = await response.text();
        throw new Error(`Failed to enhance prompt: ${text}`);
    }

    return response.json();
}

export interface ChatResponse {
    response: string;
}

/**
 * Stream chat response from the backend
 * 
 * @param message User message
 * @param onToken Callback for each new token
 * @param context Optional context
 */
export async function chatStream(
    message: string,
    onToken: (token: string) => void,
    context?: string
): Promise<void> {
    const response = await fetch(`${API_BASE}/api/chat/stream`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ message, context }),
    });

    if (!response.ok) {
        const text = await response.text();
        throw new Error(`Failed to stream chat: ${text}`);
    }

    if (!response.body) {
        throw new Error('Response body is null');
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        // Append new chunk to buffer
        buffer += decoder.decode(value, { stream: true });

        // Process complete lines from buffer
        const lines = buffer.split('\n');
        // Keep the last (potentially incomplete) line in buffer
        buffer = lines.pop() || '';

        for (const line of lines) {
            if (line.startsWith('data:')) {
                // Per SSE spec there may be an optional single space after "data:"
                // We want to remove at most one leading space, but keep any spaces
                // that are part of the actual payload (important for tokens that
                // encode leading spaces, e.g. " How").
                let data = line.slice(5);
                if (data.startsWith(' ')) {
                    data = data.slice(1);
                }
                if (data.length > 0) {
                    onToken(data);
                }
            }
        }
    }

    // Process any remaining data in buffer
    if (buffer.startsWith('data:')) {
        let data = buffer.slice(5);
        if (data.startsWith(' ')) {
            data = data.slice(1);
        }
        if (data.length > 0) {
            onToken(data);
        }
    }
}

// User Preferences
export interface UserPreferences {
    explanation_style?: string;
    code_style?: string;
    documentation_style?: string;
    test_style?: string;
    language_preference?: string;
    verbosity?: string;
}

export interface GetPreferencesResponse {
    preferences: UserPreferences;
}

export async function getPreferences(): Promise<GetPreferencesResponse> {
    const response = await fetch(`${API_BASE}/api/preferences`);

    if (!response.ok) {
        throw new Error(`Failed to get preferences: ${response.statusText}`);
    }

    return response.json();
}

export interface UpdatePreferencesRequest {
    preferences: UserPreferences;
}

export interface UpdatePreferencesResponse {
    success: boolean;
}

export async function updatePreferences(preferences: UserPreferences): Promise<UpdatePreferencesResponse> {
    const response = await fetch(`${API_BASE}/api/preferences`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ preferences }),
    });

    if (!response.ok) {
        throw new Error(`Failed to update preferences: ${response.statusText}`);
    }

    return response.json();
}

// App Status
export interface AppStatusResponse {
    status: 'initializing' | 'ready' | 'error';
    message?: string;
    progress?: string;
}

export async function getAppStatus(): Promise<AppStatusResponse> {
    const response = await fetch(`${API_BASE}/api/status`);

    if (!response.ok) {
        throw new Error(`Failed to get app status: ${response.statusText}`);
    }

    return response.json();
}

export interface RetryInitResponse {
    success: boolean;
    message: string;
}

export async function retryInit(): Promise<RetryInitResponse> {
    const response = await fetch(`${API_BASE}/api/retry_init`, {
        method: 'POST',
    });

    if (!response.ok) {
        throw new Error(`Failed to retry initialization: ${response.statusText}`);
    }

    return response.json();
}

// App Settings
export type ThemeMode = 'dark' | 'light' | 'system';

export interface AppSettings {
    intent_detection_enabled: boolean;
    llm_enabled: boolean;
    server_port?: number;
    server_address?: string;
    /** Whether anonymous analytics is enabled (default: true) */
    analytics_enabled: boolean;
    /** Theme mode (default: system) */
    theme: ThemeMode;
}

export async function getAppSettings(): Promise<AppSettings> {
    const response = await fetch(`${API_BASE}/api/settings`);
    if (!response.ok) {
        throw new Error(`Failed to get settings: ${response.statusText}`);
    }
    return response.json();
}

export async function updateAppSettings(settings: AppSettings): Promise<void> {
    const response = await fetch(`${API_BASE}/api/settings`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify(settings),
    });
    if (!response.ok) {
        throw new Error(`Failed to update settings: ${response.statusText}`);
    }
}

export async function clearAllData(): Promise<void> {
    const response = await fetch(`${API_BASE}/api/clear_all_data`, {
        method: 'POST',
    });
    if (!response.ok) {
        const text = await response.text();
        throw new Error(`Failed to clear data: ${text}`);
    }
}

// Cloudflare Worker Skills Registry API
export interface RemoteSkill {
    skill_id: string;
    url: string;
    skill: string;
    ref: string;
    content?: string;
    install_count: number;
    updated_at: string;
}

export interface ListRemoteSkillsResponse {
    success: boolean;
    skills: RemoteSkill[];
    pagination: {
        total: number;
        page: number;
        limit: number;
        total_pages: number;
    };
}

export async function listRemoteSkills(page = 1, limit = 20): Promise<ListRemoteSkillsResponse> {
    const url = `${REGISTRY_URL}/skills?page=${page}&limit=${limit}`;
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to fetch remote skills: ${response.statusText}`);
    }
    return response.json();
}

export interface SearchRemoteSkillsResponse extends ListRemoteSkillsResponse {
    query: string;
}

export async function searchRemoteSkills(query: string, page = 1, limit = 20): Promise<SearchRemoteSkillsResponse> {
    const encodedQuery = encodeURIComponent(query);
    const url = `${REGISTRY_URL}/skills/search?q=${encodedQuery}&page=${page}&limit=${limit}`;
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to search remote skills: ${response.statusText}`);
    }
    return response.json();
}

// skills.sh API
export interface SkillsShSkill {
    id: string;
    skillId: string;
    name: string;
    installs: number;
    source: string;
    topSource?: string; // Legacy support if needed
}

export interface SkillsShResponse {
    query: string;
    searchType: string;
    skills: SkillsShSkill[];
}

export async function searchSkillsSh(query: string, limit = 50): Promise<SkillsShResponse> {
    const encodedQuery = encodeURIComponent(query);
    const url = `${API_BASE}/api/skills_sh/search?q=${encodedQuery}&limit=${limit}`;
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to search skills.sh: ${response.statusText}`);
    }
    return response.json();
}



// Graph (Architect) API
export interface GraphNode {
    id: string;
    label: string;
    language: string;
    folder: string;
}

export interface GraphEdge {
    source: string;
    target: string;
    kind: string;
}

export interface GraphResponse {
    project_id: string;
    nodes: GraphNode[];
    edges: GraphEdge[];
    built_at?: string;
}

export interface GraphStatusResponse {
    status: 'missing' | 'stale' | 'ready' | 'building' | 'error';
    node_count?: number;
    edge_count?: number;
    built_at?: string;
}



export interface GraphQuery {
    folder?: string;
    focus?: string;
    hops?: number;
}

export async function getGraphStatus(sourceId: string): Promise<GraphStatusResponse> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/graph/status`);
    if (!response.ok) {
        throw new Error(`Failed to get graph status: ${response.statusText}`);
    }
    return response.json();
}

export async function getGraph(sourceId: string, query?: GraphQuery): Promise<GraphResponse> {
    const params = new URLSearchParams();
    if (query?.folder) params.set('folder', query.folder);
    if (query?.focus) params.set('focus', query.focus);
    if (query?.hops) params.set('hops', query.hops.toString());

    const url = `${API_BASE}/api/sources/${sourceId}/graph${params.toString() ? '?' + params.toString() : ''}`;
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to get graph: ${response.statusText}`);
    }
    return response.json();
}

export async function rebuildGraph(sourceId: string): Promise<GraphStatusResponse> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/graph/rebuild`, {
        method: 'POST',
    });
    if (!response.ok) {
        throw new Error(`Failed to rebuild graph: ${response.statusText}`);
    }
    return response.json();
}

// Combined graph + status response (optimized single request)
export interface GraphWithStatusResponse {
    status: string;
    node_count: number;
    edge_count: number;
    built_at: string | null;
    project_id: string;
    nodes: GraphNode[];
    edges: GraphEdge[];
}

// Get graph with status in a single request (optimized endpoint)
// Use focus parameter to get only nodes related to a specific file
// Use hops parameter to control how many relationship levels to include
export async function getGraphWithStatus(
    sourceId: string,
    query?: GraphQuery
): Promise<GraphWithStatusResponse> {
    const params = new URLSearchParams();
    if (query?.folder) params.set('folder', query.folder);
    if (query?.focus) params.set('focus', query.focus);
    if (query?.hops) params.set('hops', query.hops.toString());

    const url = `${API_BASE}/api/sources/${sourceId}/graph/with_status${params.toString() ? '?' + params.toString() : ''
        }`;
    const response = await fetch(url);
    if (!response.ok) {
        throw new Error(`Failed to get graph with status: ${response.statusText}`);
    }
    return response.json();
}



export interface RenameNoteRequest {
    old_path: string;
    new_path: string;
}

export async function renameNote(sourceId: string, oldPath: string, newPath: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/sources/${sourceId}/notes/rename`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ old_path: oldPath, new_path: newPath }),
    });

    if (!response.ok) {
        throw new Error(`Failed to rename note: ${response.statusText}`);
    }
}

// Library API
export interface LibraryPack {
    id: string;
    name: string;
    filename?: string;
    description: string;
    scope: 'Curated' | 'Team' | 'Personal';
    version: string;
    author: string;
    tags: string[];
    color?: string;
    folder?: string;
    created_at?: string;
    updated_at?: string;
    read_only?: boolean;
    file_type?: string; // File extension (md, py, js, ts, etc.)
}

export interface ListPacksResponse {
    packs: LibraryPack[];
}

export interface LibraryData {
    packs: LibraryPack[];
    folders: string[];
}

export async function getLibrary(): Promise<LibraryData> {
    const response = await fetch(`${API_BASE}/api/library`);
    if (!response.ok) {
        throw new Error(`Failed to load library: ${response.statusText}`);
    }
    return response.json();
}

export async function createPack(folder: string, name: string): Promise<{ id: string; path: string }> {
    const response = await fetch(`${API_BASE}/api/library/packs`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ folder, name }),
    });
    if (!response.ok) {
        throw new Error(`Failed to create pack: ${response.statusText}`);
    }
    return response.json();
}

export async function renamePack(packId: string, newName: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/packs/rename`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ pack_id: packId, new_name: newName }),
    });
    if (!response.ok) {
        throw new Error(`Failed to rename pack: ${response.statusText}`);
    }
}

export async function deletePack(packId: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/packs/${encodeURIComponent(packId)}`, {
        method: 'DELETE',
    });
    if (!response.ok) {
        throw new Error(`Failed to delete pack: ${response.statusText}`);
    }
}

export async function createLibraryFolder(name: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/folders`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ name }),
    });
    if (!response.ok) {
        throw new Error(`Failed to create library folder: ${response.statusText}`);
    }
}

export async function renameLibraryFolder(oldName: string, newName: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/folders/rename`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ old_name: oldName, new_name: newName }),
    });
    if (!response.ok) {
        throw new Error(`Failed to rename library folder: ${response.statusText}`);
    }
}

export async function deleteLibraryFolder(folderName: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/folders/${encodeURIComponent(folderName)}`, {
        method: 'DELETE',
    });
    if (!response.ok) {
        throw new Error(`Failed to delete library folder: ${response.statusText}`);
    }
}

export async function getPack(packId: string): Promise<{ path: string; content: string }> {
    const response = await fetch(`${API_BASE}/api/library/packs/${encodeURIComponent(packId)}`);
    if (!response.ok) {
        throw new Error(`Failed to get pack: ${response.statusText}`);
    }
    return response.json();
}

export async function savePack(packId: string, content: string): Promise<void> {
    const response = await fetch(`${API_BASE}/api/library/packs/${encodeURIComponent(packId)}`, {
        method: 'PUT',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ content }),
    });

    if (!response.ok) {
        throw new Error(`Failed to save pack: ${response.statusText}`);
    }
}

export interface DownloadSkillRequest {
    url: string;
    skill: string;
    ref: string;
}

export interface DownloadSkillResponse {
    success: boolean;
    skill: string;
    path: string;
    content?: string | null;
}

export async function downloadSkill(url: string, skill: string, ref: string = 'main'): Promise<DownloadSkillResponse> {
    const response = await fetch(`${API_BASE}/api/library/download_skill`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ url, skill, ref }),
    });

    if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Failed to download skill: ${errorText || response.statusText}`);
    }

    return response.json();
}

const INSTALL_COOLDOWN_MINUTES = 5;

interface RecordInstallResponse {
    counted: boolean;
}

function getLastInstallTime(skillId: string): Date | null {
    const key = `linggen_skill_install_${skillId}`;
    const timestamp = localStorage.getItem(key);
    if (!timestamp) return null;
    return new Date(timestamp);
}

function saveInstallTime(skillId: string): void {
    const key = `linggen_skill_install_${skillId}`;
    localStorage.setItem(key, new Date().toISOString());
}

export async function recordSkillInstall(
    url: string,
    skill: string,
    ref: string,
    skillId: string,
    content?: string | null
): Promise<boolean> {
    // Check local cooldown to prevent spam
    const lastInstall = getLastInstallTime(skillId);
    if (lastInstall) {
        const elapsed = Date.now() - lastInstall.getTime();
        const cooldownMs = INSTALL_COOLDOWN_MINUTES * 60 * 1000;
        if (elapsed < cooldownMs) {
            console.log(`[Install Tracking] Cooldown active for skill ${skill}. Skipping.`);
            return false;
        }
    }

    try {
        if (!REGISTRY_API_KEY) {
            console.warn('[Install Tracking] Missing VITE_LINGGEN_CF_WORKER_API_KEY; skipping.');
            return false;
        }

        const payload: Record<string, unknown> = {
            url: url,
            skill: skill,
            ref: ref,
            installer: 'linggen-web',
            installer_version: '1.0.0',
            timestamp: new Date().toISOString(),
        };
        if (content) payload.content = content;

        const response = await fetch(`${REGISTRY_URL}/skills/install`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-API-Key': REGISTRY_API_KEY,
            },
            body: JSON.stringify(payload),
        });

        if (!response.ok) {
            console.warn('[Install Tracking] Failed to record install:', response.statusText);
            return false;
        }

        const result: RecordInstallResponse = await response.json();
        if (result.counted) {
            saveInstallTime(skillId);
            console.log(`[Install Tracking] Install recorded and counted for ${skill}`);
        } else {
            console.log(`[Install Tracking] Install recorded but not counted (recently counted by server)`);
        }
        return result.counted;
    } catch (error) {
        console.warn('[Install Tracking] Failed to record install:', error);
        return false;
    }
}
