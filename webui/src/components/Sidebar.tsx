import {
    FolderIcon,
    ClockIcon,
    Cog6ToothIcon,
    FolderPlusIcon,
    DocumentPlusIcon,
    ChevronRightIcon,
    ChevronDownIcon,
    PencilIcon,
    TrashIcon,
    QuestionMarkCircleIcon
} from '@heroicons/react/24/outline'
import { useState, useEffect } from 'react';
import {
    type Resource,
    type LibraryPack,
    saveNote,
    listNotes,
    renameNote,
    deleteNote,
    removeResource,
    type Note,
    listMemoryFiles,
    type MemoryFile,
    createPack,
    renamePack,
    deletePack,
    createLibraryFolder,
    renameLibraryFolder,
    deleteLibraryFolder,
} from '../api'
import { ContextMenu, ContextMenuItem } from './ContextMenu';

export type View = 'sources' | 'library' | 'activity' | 'assistant' | 'settings'

interface SidebarProps {
    currentView: View
    onChangeView: (view: View) => void
    resources?: Resource[]
    resourcesVersion?: number
    selectedSourceId?: string | null
    onSelectSource?: (id: string | null) => void
    selectedNotePath?: string | null
    onSelectNote?: (sourceId: string, path: string) => void
    selectedMemoryPath?: string | null
    onSelectMemory?: (sourceId: string, path: string) => void
    selectedLibraryPackId?: string | null
    onSelectLibraryPack?: (packId: string | null) => void
    onAddSource?: () => void
    libraryPacks?: LibraryPack[]
    libraryFolders?: string[]
    onRefresh?: () => void
}

interface ContextMenuState {
    x: number;
    y: number;
    sourceId?: string;
    notePath?: string;
    libraryPackId?: string;
    libraryFolder?: string;
}

interface DeleteSourceConfirmation {
    sourceId: string;
    sourceName: string;
}

export function Sidebar({
    currentView,
    onChangeView,
    resources = [],
    resourcesVersion = 0,
    selectedSourceId,
    onSelectSource,
    selectedNotePath,
    onSelectNote,
    selectedMemoryPath,
    onSelectMemory,
    selectedLibraryPackId,
    onSelectLibraryPack,
    onAddSource,
    libraryPacks = [],
    libraryFolders = [],
    onRefresh
}: SidebarProps) {
    const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
    const [creatingNote, setCreatingNote] = useState<string | null>(null); // sourceId where we are creating a note
    const [creatingLibraryPack, setCreatingLibraryPack] = useState<string | null>(null); // folder where we are creating a pack
    const [creatingLibraryFolder, setCreatingLibraryFolder] = useState<string | null>(null); // parent folder where we are creating a folder

    // renaming: { type: 'note' | 'source' | 'libraryPack' | 'libraryFolder', id: string, oldPath: string }
    const [renamingNote, setRenamingNote] = useState<{ sourceId: string, oldPath: string } | null>(null);
    const [renamingLibraryPack, setRenamingLibraryPack] = useState<{ id: string, oldName: string } | null>(null);
    const [renamingLibraryFolder, setRenamingLibraryFolder] = useState<{ oldName: string } | null>(null);

    const [deleteConfirmation, setDeleteConfirmation] = useState<{ sourceId: string, notePath: string } | null>(null);
    const [deleteSourceConfirmation, setDeleteSourceConfirmation] = useState<DeleteSourceConfirmation | null>(null);
    const [deleteLibraryPackConfirmation, setDeleteLibraryPackConfirmation] = useState<{ id: string, name: string } | null>(null);
    const [deleteLibraryFolderConfirmation, setDeleteLibraryFolderConfirmation] = useState<{ name: string } | null>(null);
    const [expandedSources, setExpandedSources] = useState<Set<string>>(new Set());
    const [expandedMemories, setExpandedMemories] = useState<Set<string>>(new Set());
    const [expandedLibraryFolders, setExpandedLibraryFolders] = useState<Set<string>>(new Set(['skills', 'policies']));
    const [sourceNotes, setSourceNotes] = useState<Record<string, Note[]>>({});
    const [sourceMemories, setSourceMemories] = useState<Record<string, MemoryFile[]>>({});

    // Collapsible sections state
    const [isProjectsCollapsed, setIsProjectsCollapsed] = useState(false);
    const [isLibraryCollapsed, setIsLibraryCollapsed] = useState(false);

    // Build library tree structure
    interface LibraryTreeNode {
        name: string;
        path: string;
        children: Record<string, LibraryTreeNode>;
        packs: LibraryPack[];
        isOfficial: boolean;
    }

    const buildLibraryTree = () => {
        const tree: Record<string, LibraryTreeNode> = {};

        const getOrCreateNode = (parts: string[], root: Record<string, LibraryTreeNode>): LibraryTreeNode => {
            let current = root;
            let fullPath = '';
            let node: LibraryTreeNode | undefined;

            for (let i = 0; i < parts.length; i++) {
                const part = parts[i];
                fullPath = fullPath ? `${fullPath}/${part}` : part;
                if (!current[part]) {
                    current[part] = {
                        name: part,
                        path: fullPath,
                        children: {},
                        packs: [],
                        isOfficial: false
                    };
                }
                node = current[part];
                current = node.children;
            }
            return node!;
        };

        // Initialize folders from the API list
        libraryFolders.forEach(folderPath => {
            const parts = folderPath.split('/').filter(Boolean);
            if (parts.length > 0) {
                getOrCreateNode(parts, tree);
            }
        });

        // Assign packs to nodes
        libraryPacks.forEach(pack => {
            const folderPath = pack.folder || 'general';
            const parts = folderPath.split('/').filter(Boolean);
            const node = getOrCreateNode(parts, tree);
            node.packs.push(pack);
        });

        return tree;
    };

    const libraryTree = buildLibraryTree();

    const toggleLibraryFolder = (folder: string) => {
        const newSet = new Set(expandedLibraryFolders);
        if (newSet.has(folder)) newSet.delete(folder);
        else newSet.add(folder);
        setExpandedLibraryFolders(newSet);
    };

    // Refresh notes for expanded sources periodically or when resources/version change
    useEffect(() => {
        resources.forEach(resource => {
            if (expandedSources.has(resource.id)) {
                loadNotes(resource.id);
                loadMemories(resource.id);
            }
        });
    }, [expandedSources, resources, resourcesVersion]);

    const loadNotes = async (sourceId: string) => {
        try {
            const notes = await listNotes(sourceId);
            setSourceNotes(prev => ({
                ...prev,
                [sourceId]: notes
            }));
        } catch (error) {
            console.error(`Failed to load notes for source ${sourceId}`, error);
        }
    };

    const loadMemories = async (sourceId: string) => {
        try {
            const files = await listMemoryFiles(sourceId);
            setSourceMemories(prev => ({
                ...prev,
                [sourceId]: files
            }));
            // Auto-expand memories on first load if there are files and the user hasn't toggled yet.
            if (files.length > 0) {
                setExpandedMemories(prev => {
                    if (prev.has(sourceId)) return prev;
                    const next = new Set(prev);
                    next.add(sourceId);
                    return next;
                });
            }
        } catch (error) {
            console.error(`Failed to load memories for source ${sourceId}`, error);
        }
    };

    const toggleSourceExpansion = async (e: React.MouseEvent, sourceId: string) => {
        e.stopPropagation();
        const isExpanded = expandedSources.has(sourceId);
        const newExpanded = new Set(expandedSources);
        if (isExpanded) {
            newExpanded.delete(sourceId);
        } else {
            newExpanded.add(sourceId);
            loadNotes(sourceId);
            loadMemories(sourceId);
        }
        setExpandedSources(newExpanded);
    };


    const handleSourceClick = (id: string) => {
        onChangeView('sources')
        onSelectSource?.(id)
    }

    const handleMemoryClick = (e: React.MouseEvent, sourceId: string, path: string) => {
        e.stopPropagation();
        onChangeView('sources');
        if (selectedSourceId !== sourceId) {
            onSelectSource?.(sourceId);
        }
        onSelectMemory?.(sourceId, path);
    }

    const toggleMemoriesExpansion = (e: React.MouseEvent, sourceId: string) => {
        e.stopPropagation();
        setExpandedMemories(prev => {
            const next = new Set(prev);
            if (next.has(sourceId)) {
                next.delete(sourceId);
            } else {
                next.add(sourceId);
            }
            return next;
        });
    };

    const handleContextMenu = (e: React.MouseEvent, sourceId: string, notePath?: string) => {
        e.preventDefault();
        e.stopPropagation();
        e.nativeEvent.stopImmediatePropagation();
        setContextMenu({
            x: e.clientX,
            y: e.clientY,
            sourceId,
            notePath
        });
    };

    const startCreatingNote = (sourceId: string) => {
        setCreatingNote(sourceId);
        if (!expandedSources.has(sourceId)) {
            setExpandedSources(prev => new Set(prev).add(sourceId));
            loadNotes(sourceId);
        }
    };

    const handleCreateNoteSubmit = async (sourceId: string, filename: string) => {
        if (!filename.trim()) {
            setCreatingNote(null);
            return;
        }

        if (!filename.endsWith('.md')) {
            filename += '.md';
        }

        try {
            await saveNote(sourceId, filename, "# " + filename.replace('.md', '') + "\n\nStart writing...");
            console.log(`Created markdown: ${filename}`);
            await loadNotes(sourceId);
        } catch (err) {
            console.error("Failed to create note:", err);
            alert("Failed to create note.");
        } finally {
            setCreatingNote(null);
        }
    };

    const handleRenameNoteTrigger = () => {
        if (!contextMenu?.notePath || !contextMenu?.sourceId) return;
        setRenamingNote({
            sourceId: contextMenu.sourceId,
            oldPath: contextMenu.notePath
        });
        setContextMenu(null);
    };

    const handleRenameSubmit = async (newName: string) => {
        if (!renamingNote || !newName.trim()) {
            setRenamingNote(null);
            return;
        }

        if (!newName.endsWith('.md')) {
            newName += '.md';
        }

        if (newName === renamingNote.oldPath) {
            setRenamingNote(null);
            return;
        }

        try {
            await renameNote(renamingNote.sourceId, renamingNote.oldPath, newName);
            await loadNotes(renamingNote.sourceId);
        } catch (err) {
            console.error("Failed to rename note:", err);
            alert("Failed to rename note.");
        } finally {
            setRenamingNote(null);
        }
    };

    const handleDeleteNote = () => {
        if (!contextMenu?.notePath || !contextMenu?.sourceId) return;
        const { sourceId, notePath } = contextMenu;
        setContextMenu(null);
        setDeleteConfirmation({ sourceId, notePath });
    };

    const handleConfirmDelete = async () => {
        if (!deleteConfirmation) return;
        const { sourceId, notePath } = deleteConfirmation;

        try {
            await deleteNote(sourceId, notePath);
            await loadNotes(sourceId);

            // Deselect if currently selected
            if (selectedNotePath === notePath && selectedSourceId === sourceId) {
                onSelectNote?.(sourceId, '');
            }
        } catch (err) {
            console.error("Failed to delete note:", err);
            alert(`Failed to delete note: ${err instanceof Error ? err.message : String(err)}`);
        } finally {
            setDeleteConfirmation(null);
        }
    };

    const handleAddMarkdown = () => {
        if (!contextMenu || !contextMenu.sourceId) return;
        const { sourceId } = contextMenu;
        setContextMenu(null);
        startCreatingNote(sourceId);
    };

    const handleRemoveSource = () => {
        if (!contextMenu || !contextMenu.sourceId || contextMenu.notePath) return; // Only for sources, not notes
        const { sourceId } = contextMenu;
        const resource = resources.find(r => r.id === sourceId);
        setContextMenu(null);

        if (resource) {
            setDeleteSourceConfirmation({
                sourceId,
                sourceName: resource.name
            });
        }
    };

    const handleConfirmRemoveSource = async () => {
        if (!deleteSourceConfirmation) return;
        const { sourceId } = deleteSourceConfirmation;

        try {
            await removeResource(sourceId);

            // Deselect if currently selected
            if (selectedSourceId === sourceId) {
                onSelectSource?.(null);
            }

            // Collapse if expanded
            if (expandedSources.has(sourceId)) {
                const newExpanded = new Set(expandedSources);
                newExpanded.delete(sourceId);
                setExpandedSources(newExpanded);
            }

            // Clean up local caches for removed source
            setSourceNotes(prev => {
                const next = { ...prev };
                delete next[sourceId];
                return next;
            });
            setSourceMemories(prev => {
                const next = { ...prev };
                delete next[sourceId];
                return next;
            });
            setExpandedMemories(prev => {
                const next = new Set(prev);
                next.delete(sourceId);
                return next;
            });

            // Refresh resources list so UI updates immediately (no manual reload needed)
            onRefresh?.();
        } catch (err) {
            console.error("Failed to remove source:", err);
            alert(`Failed to remove source: ${err instanceof Error ? err.message : String(err)}`);
        } finally {
            setDeleteSourceConfirmation(null);
        }
    };

    const handleHeaderAddMarkdown = () => {
        if (!selectedSourceId) {
            alert("Please select a source first to add a markdown note.");
            return;
        }
        startCreatingNote(selectedSourceId);
    };

    const handleNoteClick = (e: React.MouseEvent, sourceId: string, notePath: string) => {
        e.stopPropagation();
        onChangeView('sources');
        if (selectedSourceId !== sourceId) {
            onSelectSource?.(sourceId);
        }
        onSelectNote?.(sourceId, notePath);
    };

    const handleLibraryContextMenu = (e: React.MouseEvent, packId?: string, folder?: string) => {
        e.preventDefault();
        e.stopPropagation();
        e.nativeEvent.stopImmediatePropagation();

        setContextMenu({
            x: e.clientX,
            y: e.clientY,
            libraryPackId: packId,
            libraryFolder: folder
        });
    };

    const handleCreateLibraryPackSubmit = async (folder: string, name: string) => {
        if (!name.trim()) {
            setCreatingLibraryPack(null);
            return;
        }

        try {
            await createPack(folder, name);
            onRefresh?.();
        } catch (err) {
            console.error("Failed to create library pack:", err);
            alert("Failed to create library pack.");
        } finally {
            setCreatingLibraryPack(null);
        }
    };

    const handleCreateLibraryFolderSubmit = async (parentPath: string | null, name: string) => {
        if (!name.trim()) {
            setCreatingLibraryFolder(null);
            return;
        }

        const fullPath = parentPath ? `${parentPath}/${name}` : name;

        try {
            await createLibraryFolder(fullPath);
            onRefresh?.();
        } catch (err) {
            console.error("Failed to create library folder:", err);
            alert("Failed to create library folder.");
        } finally {
            setCreatingLibraryFolder(null);
        }
    };

    const handleRenameLibraryPackSubmit = async (newName: string) => {
        if (!renamingLibraryPack || !newName.trim()) {
            setRenamingLibraryPack(null);
            return;
        }

        if (newName === renamingLibraryPack.oldName) {
            setRenamingLibraryPack(null);
            return;
        }

        try {
            await renamePack(renamingLibraryPack.id, newName);
            onRefresh?.();
        } catch (err) {
            console.error("Failed to rename library pack:", err);
            alert("Failed to rename library pack.");
        } finally {
            setRenamingLibraryPack(null);
        }
    };

    const handleRenameLibraryFolderSubmit = async (newName: string) => {
        if (!renamingLibraryFolder || !newName.trim()) {
            setRenamingLibraryFolder(null);
            return;
        }

        if (newName === renamingLibraryFolder.oldName) {
            setRenamingLibraryFolder(null);
            return;
        }

        try {
            await renameLibraryFolder(renamingLibraryFolder.oldName, newName);
            onRefresh?.();
        } catch (err) {
            console.error("Failed to rename library folder:", err);
            alert("Failed to rename library folder.");
        } finally {
            setRenamingLibraryFolder(null);
        }
    };

    const handleConfirmDeleteLibraryPack = async () => {
        if (!deleteLibraryPackConfirmation) return;
        try {
            await deletePack(deleteLibraryPackConfirmation.id);
            onRefresh?.();
            if (selectedLibraryPackId === deleteLibraryPackConfirmation.id) {
                onSelectLibraryPack?.('');
            }
        } catch (err) {
            console.error("Failed to delete library pack:", err);
            alert("Failed to delete library pack.");
        } finally {
            setDeleteLibraryPackConfirmation(null);
        }
    };

    const handleConfirmDeleteLibraryFolder = async () => {
        if (!deleteLibraryFolderConfirmation) return;
        try {
            await deleteLibraryFolder(deleteLibraryFolderConfirmation.name);
            onRefresh?.();
        } catch (err) {
            console.error("Failed to delete library folder:", err);
            alert("Failed to delete library folder.");
        } finally {
            setDeleteLibraryFolderConfirmation(null);
        }
    };

    const SidebarSectionHeader = ({
        label,
        isCollapsed,
        onToggle,
        onClick,
        actions,
        isActive
    }: {
        label: string,
        isCollapsed: boolean,
        onToggle: () => void,
        onClick?: () => void,
        actions?: React.ReactNode,
        isActive?: boolean
    }) => (
        <div
            className={`px-4 py-1.5 flex justify-between items-center mt-2 mb-1 cursor-pointer select-none group transition-colors ${isActive ? 'bg-[var(--item-hover)] shadow-[inset_3px_0_0_0_var(--accent)]' : 'hover:bg-[var(--item-hover)]/50'}`}
            onClick={onClick || onToggle}
        >
            <div className="flex items-center gap-1.5">
                <div 
                    className="w-3 h-3 flex items-center justify-center hover:text-[var(--text-active)] transition-colors"
                    onClick={(e) => {
                        if (onClick) {
                            e.stopPropagation();
                            onToggle();
                        }
                    }}
                >
                    {isCollapsed ? (
                        <ChevronRightIcon className="w-2.5 h-2.5 text-[var(--text-secondary)]" />
                    ) : (
                        <ChevronDownIcon className="w-2.5 h-2.5 text-[var(--text-secondary)]" />
                    )}
                </div>
                <span className={`text-[11px] font-bold tracking-wider ${isActive ? 'text-[var(--text-active)]' : 'text-[var(--text-secondary)]'}`}>
                    {label}
                </span>
            </div>

            {actions && (
                <div className="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity" onClick={e => e.stopPropagation()}>
                    {actions}
                </div>
            )}
        </div>
    );

    // Helper to get file type icon text and color
    const getFileTypeIcon = (pack: LibraryPack): { text: string, color: string } => {
        const fileType = pack.file_type?.toLowerCase();

        switch (fileType) {
            case 'md':
                return { text: 'M', color: '#60a5fa' }; // blue
            case 'py':
                return { text: 'PY', color: '#fbbf24' }; // yellow/amber
            case 'js':
                return { text: 'JS', color: '#facc15' }; // yellow
            case 'jsx':
                return { text: 'JSX', color: '#facc15' }; // yellow
            case 'ts':
                return { text: 'TS', color: '#3b82f6' }; // blue
            case 'tsx':
                return { text: 'TSX', color: '#3b82f6' }; // blue
            case 'sh':
            case 'bash':
            case 'zsh':
                return { text: 'SH', color: '#10b981' }; // green
            default:
                return { text: 'L', color: pack.color || '#A78BFA' }; // fallback to library purple
        }
    };

    const renderLibraryNode = (node: LibraryTreeNode, depth: number) => {
        const isExpanded = expandedLibraryFolders.has(node.path);
        const isRenamingFolder = node.path && renamingLibraryFolder?.oldName === node.path;
        const totalItems = Object.keys(node.children).length + node.packs.length;

        return (
            <div key={node.path} className="flex flex-col">
                {isRenamingFolder ? (
                    <div className="sidebar-item text-[0.85rem] w-full flex items-center gap-1.5" style={{ paddingLeft: `${(depth + 1) * 16}px` }}>
                        <FolderIcon className="w-3.5 h-3.5 text-[var(--text-muted)]" />
                        <input
                            autoFocus
                            type="text"
                            defaultValue={node.name}
                            className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                            onKeyDown={(e) => {
                                if (e.key === 'Enter') {
                                    e.preventDefault();
                                    e.stopPropagation();
                                    handleRenameLibraryFolderSubmit(e.currentTarget.value);
                                } else if (e.key === 'Escape') {
                                    e.preventDefault();
                                    e.stopPropagation();
                                    setRenamingLibraryFolder(null);
                                }
                            }}
                            onBlur={() => {
                                setTimeout(() => setRenamingLibraryFolder(null), 200);
                            }}
                        />
                    </div>
                ) : (
                    <button
                        className="sidebar-item text-[0.7rem] w-full flex items-center gap-1.5 text-[var(--text-muted)] bg-transparent uppercase tracking-wider opacity-90"
                        style={{ paddingLeft: `${(depth + 1) * 16}px` }}
                        onClick={() => toggleLibraryFolder(node.path)}
                        onContextMenu={(e) => handleLibraryContextMenu(e, undefined, node.path)}
                    >
                        {isExpanded ? (
                            <ChevronDownIcon className="w-3 h-3" />
                        ) : (
                            <ChevronRightIcon className="w-3 h-3" />
                        )}
                        <span className="flex-1 text-left">{node.name}</span>
                        {totalItems > 0 && (
                            <span className="text-[0.65rem] opacity-80">
                                {totalItems}
                            </span>
                        )}
                    </button>
                )}
                {isExpanded && (
                    <div className="flex flex-col">
                        {/* Recursive children folders */}
                        {Object.values(node.children)
                            .sort((a, b) => a.name.localeCompare(b.name))
                            .map(child => renderLibraryNode(child, depth + 1))}

                        {/* Creating subfolder input */}
                        {creatingLibraryFolder === node.path && (
                            <div className="sidebar-item text-[0.85rem] w-full flex items-center gap-1.5" style={{ paddingLeft: `${(depth + 2) * 16}px` }}>
                                <FolderIcon className="w-3.5 h-3.5 text-[var(--text-muted)]" />
                                <input
                                    autoFocus
                                    type="text"
                                    defaultValue="new-folder"
                                    className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter') {
                                            e.preventDefault();
                                            e.stopPropagation();
                                            handleCreateLibraryFolderSubmit(node.path, e.currentTarget.value);
                                        } else if (e.key === 'Escape') {
                                            e.preventDefault();
                                            e.stopPropagation();
                                            setCreatingLibraryFolder(null);
                                        }
                                    }}
                                    onBlur={() => {
                                        // Delay blur to allow Enter key to process first if needed
                                        // or just ignore if we are about to unmount anyway
                                        setTimeout(() => setCreatingLibraryFolder(null), 200);
                                    }}
                                />
                            </div>
                        )}

                        {/* Creating pack input */}
                        {creatingLibraryPack === node.path && (
                            <div className="sidebar-item text-[0.85rem] w-full flex items-center gap-1.5" style={{ paddingLeft: `${(depth + 2) * 16}px` }}>
                                <div className="text-[10px] font-bold text-blue-400 border border-blue-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">L</div>
                                <input
                                    autoFocus
                                    type="text"
                                    defaultValue="New Pack"
                                    className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                    onKeyDown={(e) => {
                                        if (e.key === 'Enter') {
                                            e.preventDefault();
                                            e.stopPropagation();
                                            handleCreateLibraryPackSubmit(node.path, e.currentTarget.value);
                                        } else if (e.key === 'Escape') {
                                            e.preventDefault();
                                            e.stopPropagation();
                                            setCreatingLibraryPack(null);
                                        }
                                    }}
                                    onBlur={() => {
                                        setTimeout(() => setCreatingLibraryPack(null), 200);
                                    }}
                                />
                            </div>
                        )}

                        {/* Packs in this folder */}
                        {node.packs.map(pack => {
                            const isRenaming = pack.id && renamingLibraryPack?.id === pack.id;
                            const iconInfo = getFileTypeIcon(pack);
                            return isRenaming ? (
                                <div
                                    key={pack.id || 'renaming'}
                                    className="sidebar-item text-[0.85rem] w-full flex items-center gap-1.5"
                                    style={{ paddingLeft: `${(depth + 2) * 16}px` }}
                                >
                                    <div
                                        className="text-[10px] font-bold border rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none"
                                        style={{ color: iconInfo.color, borderColor: iconInfo.color, fontSize: iconInfo.text.length > 1 ? '7px' : '10px' }}
                                    >{iconInfo.text}</div>
                                    <input
                                        autoFocus
                                        type="text"
                                        defaultValue={pack.filename || pack.name}
                                        className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                        onKeyDown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                handleRenameLibraryPackSubmit(e.currentTarget.value);
                                            } else if (e.key === 'Escape') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                setRenamingLibraryPack(null);
                                            }
                                        }}
                                        onBlur={() => {
                                            setTimeout(() => setRenamingLibraryPack(null), 200);
                                        }}
                                    />
                                </div>
                            ) : (
                                <button
                                    key={pack.id}
                                    className={`sidebar-item text-[0.85rem] w-full flex items-center gap-1.5 transition-colors ${selectedLibraryPackId === pack.id ? 'active' : 'text-[var(--text-secondary)] opacity-90'}`}
                                    style={{ paddingLeft: `${(depth + 2) * 16}px` }}
                                    onClick={() => pack.id && onSelectLibraryPack?.(pack.id)}
                                    onContextMenu={(e) => pack.id && handleLibraryContextMenu(e, pack.id)}
                                >
                                    <div
                                        className="text-[10px] font-bold border rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none"
                                        style={{ color: iconInfo.color, borderColor: iconInfo.color, fontSize: iconInfo.text.length > 1 ? '7px' : '10px' }}
                                    >
                                        {iconInfo.text}
                                    </div>
                                    <span className="overflow-hidden text-ellipsis whitespace-nowrap">
                                        {pack.filename || pack.name}
                                    </span>
                                </button>
                            );
                        })}
                    </div>
                )}
            </div>
        );
    };

    return (
        <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
            <div className="flex-1 overflow-y-auto py-2.5 flex flex-col min-h-0">
                <div className="flex flex-col mb-4">
                    <SidebarSectionHeader
                        label="PROJECTS"
                        isCollapsed={isProjectsCollapsed}
                        onToggle={() => {
                            setIsProjectsCollapsed(!isProjectsCollapsed);
                            onChangeView('sources');
                            onSelectSource?.(null);
                        }}
                        isActive={currentView === 'sources' && !selectedSourceId && !selectedLibraryPackId}
                        actions={
                            <>
                                <button
                                    className="btn-ghost p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-active)]"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        handleHeaderAddMarkdown();
                                    }}
                                    title="Add Doc"
                                >
                                    <DocumentPlusIcon className="w-4.5 h-4.5" />
                                </button>
                                <button
                                    className="btn-ghost p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-active)]"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        onAddSource?.();
                                    }}
                                    title="Add Project"
                                >
                                    <FolderPlusIcon className="w-4.5 h-4.5" />
                                </button>
                            </>
                        }
                    />

                    {!isProjectsCollapsed && resources.map(resource => {
                        const notesCount = sourceNotes[resource.id]?.length || 0;
                        const memoriesCount = sourceMemories[resource.id]?.length || 0;
                        const totalItems = notesCount + memoriesCount;
                        const isExpanded = expandedSources.has(resource.id);

                        return (
                            <div
                                key={resource.id}
                                onContextMenu={(e) => handleContextMenu(e, resource.id)}
                                className="flex flex-col"
                            >
                                <button
                                    className={`sidebar-item pl-4 text-[0.7rem] w-full flex items-center gap-1.5 uppercase tracking-wider transition-colors ${selectedSourceId === resource.id && currentView === 'sources' && !selectedNotePath && !selectedMemoryPath ? 'active' : 'text-[var(--text-muted)] opacity-90'}`}
                                    onClick={() => handleSourceClick(resource.id)}
                                >
                                    <div
                                        className="p-0.5 hover:text-[var(--text-active)] cursor-pointer"
                                        onClick={(e) => toggleSourceExpansion(e, resource.id)}
                                    >
                                        {isExpanded ? (
                                            <ChevronDownIcon className="w-3 h-3" />
                                        ) : (
                                            <ChevronRightIcon className="w-3 h-3" />
                                        )}
                                    </div>

                                    <span className="overflow-hidden text-ellipsis whitespace-nowrap flex-1 text-left">
                                        {resource.name}
                                    </span>
                                    {totalItems > 0 && (
                                        <span className="text-[0.65rem] opacity-80">
                                            {totalItems}
                                        </span>
                                    )}
                                </button>

                                {isExpanded && (
                                    <div className="flex flex-col pl-2">
                                        {creatingNote === resource.id && (
                                            <div className="sidebar-item pl-8 text-[0.85rem] w-full flex items-center gap-1.5">
                                                <div className="text-[10px] font-bold text-blue-400 border border-blue-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">M</div>
                                                <input
                                                    autoFocus
                                                    type="text"
                                                    defaultValue="New Note.md"
                                                    className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                                    onKeyDown={(e) => {
                                                        if (e.key === 'Enter') {
                                                            handleCreateNoteSubmit(resource.id, e.currentTarget.value);
                                                        } else if (e.key === 'Escape') {
                                                            setCreatingNote(null);
                                                        }
                                                        e.stopPropagation();
                                                    }}
                                                    onBlur={() => {
                                                        setCreatingNote(null);
                                                    }}
                                                    onClick={(e) => e.stopPropagation()}
                                                />
                                            </div>
                                        )}
                                        {sourceNotes[resource.id]?.map((note) => (
                                            renamingNote?.sourceId === resource.id && renamingNote.oldPath === note.path ? (
                                                <div
                                                    key={note.path}
                                                    className="sidebar-item pl-8 text-[0.85rem] w-full flex items-center gap-1.5"
                                                    onClick={(e) => e.stopPropagation()}
                                                >
                                                    <div className="text-[10px] font-bold text-blue-400 border border-blue-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">M</div>
                                                    <input
                                                        autoFocus
                                                        type="text"
                                                        defaultValue={note.name}
                                                        className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                                        onKeyDown={(e) => {
                                                            if (e.key === 'Enter') {
                                                                handleRenameSubmit(e.currentTarget.value);
                                                            } else if (e.key === 'Escape') {
                                                                setRenamingNote(null);
                                                            }
                                                            e.stopPropagation();
                                                        }}
                                                        onBlur={() => {
                                                            setRenamingNote(null);
                                                        }}
                                                        onClick={(e) => e.stopPropagation()}
                                                    />
                                                </div>
                                            ) : (
                                                <button
                                                    key={note.path}
                                                    className={`sidebar-item pl-8 text-[0.85rem] w-full flex items-center gap-1.5 transition-colors ${selectedNotePath === note.path && selectedSourceId === resource.id ? 'active' : 'text-[var(--text-secondary)] opacity-90'}`}
                                                    onClick={(e) => handleNoteClick(e, resource.id, note.path)}
                                                    onContextMenu={(e) => handleContextMenu(e, resource.id, note.path)}
                                                >
                                                    <div className="text-[10px] font-bold text-blue-400 border border-blue-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">M</div>
                                                    <span className="overflow-hidden text-ellipsis whitespace-nowrap">
                                                        {note.name}
                                                    </span>
                                                </button>
                                            )
                                        ))}

                                        {/* Memories */}
                                        {sourceMemories[resource.id]?.length ? (
                                            <>
                                                <button
                                                    className="sidebar-item pl-8 mt-1.5 mb-1 text-[0.7rem] w-full flex items-center gap-1.5 text-[var(--text-muted)] bg-transparent uppercase tracking-wider opacity-90"
                                                    onClick={(e) => toggleMemoriesExpansion(e, resource.id)}
                                                    title={expandedMemories.has(resource.id) ? 'Collapse memories' : 'Expand memories'}
                                                >
                                                    {expandedMemories.has(resource.id) ? (
                                                        <ChevronDownIcon className="w-3.5 h-3.5" />
                                                    ) : (
                                                        <ChevronRightIcon className="w-3.5 h-3.5" />
                                                    )}
                                                    <span className="flex-1 text-left">Memories</span>
                                                    <span className="text-[0.65rem] opacity-80">
                                                        {sourceMemories[resource.id]?.length || 0}
                                                    </span>
                                                </button>

                                                {expandedMemories.has(resource.id) &&
                                                    sourceMemories[resource.id]?.map((mem) => (
                                                        <button
                                                            key={mem.path}
                                                            className={`sidebar-item pl-12 text-[0.85rem] w-full flex items-center gap-1.5 transition-colors ${selectedMemoryPath === mem.path && selectedSourceId === resource.id ? 'active' : 'text-[var(--text-secondary)] opacity-90'}`}
                                                            onClick={(e) => handleMemoryClick(e, resource.id, mem.path)}
                                                        >
                                                            <div className="text-[10px] font-bold text-purple-400 border border-purple-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">M</div>
                                                            <span className="overflow-hidden text-ellipsis whitespace-nowrap">
                                                                {mem.name}
                                                            </span>
                                                        </button>
                                                    ))}
                                            </>
                                        ) : null}
                                    </div>
                                )}
                            </div>
                        );
                    })}
                </div>

                <div className="flex flex-col mb-4">
                    <SidebarSectionHeader
                        label="LIBRARY"
                        isCollapsed={isLibraryCollapsed}
                        onToggle={() => {
                            setIsLibraryCollapsed(!isLibraryCollapsed);
                        }}
                        onClick={() => {
                            onChangeView('library');
                            onSelectLibraryPack?.(null);
                        }}
                        isActive={currentView === 'library' && !selectedLibraryPackId}
                        actions={
                            <>
                                <button
                                    className="btn-ghost p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-active)]"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        setCreatingLibraryPack("");
                                        if (isLibraryCollapsed) setIsLibraryCollapsed(false);
                                    }}
                                    title="Add Library File"
                                >
                                    <DocumentPlusIcon className="w-4.5 h-4.5" />
                                </button>
                                <button
                                    className="btn-ghost p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-active)]"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        setCreatingLibraryFolder("");
                                        if (isLibraryCollapsed) setIsLibraryCollapsed(false);
                                    }}
                                    title="Add Library Folder"
                                >
                                    <FolderPlusIcon className="w-4.5 h-4.5" />
                                </button>
                            </>
                        }
                    />

                    {!isLibraryCollapsed && (
                        <div className="px-2">
                            {creatingLibraryFolder === "" && (
                                <div className="sidebar-item pl-4 text-[0.85rem] w-full flex items-center gap-1.5">
                                    <FolderIcon className="w-3.5 h-3.5 text-[var(--text-muted)]" />
                                    <input
                                        autoFocus
                                        type="text"
                                        defaultValue="new-folder"
                                        className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                        onKeyDown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                handleCreateLibraryFolderSubmit("", e.currentTarget.value);
                                            } else if (e.key === 'Escape') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                setCreatingLibraryFolder(null);
                                            }
                                        }}
                                        onBlur={() => {
                                            setTimeout(() => setCreatingLibraryFolder(null), 200);
                                        }}
                                    />
                                </div>
                            )}
                            {creatingLibraryPack === "" && (
                                <div className="sidebar-item pl-4 text-[0.85rem] w-full flex items-center gap-1.5">
                                    <div className="text-[10px] font-bold text-blue-400 border border-blue-400 rounded-[2px] w-3.5 h-3.5 flex items-center justify-center leading-none">L</div>
                                    <input
                                        autoFocus
                                        type="text"
                                        defaultValue="New Pack"
                                        className="bg-[var(--bg-app)] border border-[var(--border-color)] rounded-[2px] text-[var(--text-primary)] text-inherit w-full outline-none px-1 py-0"
                                        onKeyDown={(e) => {
                                            if (e.key === 'Enter') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                handleCreateLibraryPackSubmit("", e.currentTarget.value);
                                            } else if (e.key === 'Escape') {
                                                e.preventDefault();
                                                e.stopPropagation();
                                                setCreatingLibraryPack(null);
                                            }
                                        }}
                                        onBlur={() => {
                                            setTimeout(() => setCreatingLibraryPack(null), 200);
                                        }}
                                    />
                                </div>
                            )}
                            {Object.values(libraryTree).sort((a, b) => a.name.localeCompare(b.name)).map(node => renderLibraryNode(node, 0))}
                            {libraryPacks.length === 0 && !creatingLibraryFolder && Object.keys(libraryTree).length === 0 && (
                                <div className="px-4 py-2 text-[11px] text-[var(--text-muted)] italic">
                                    No library packs found.
                                </div>
                            )}
                        </div>
                    )}
                </div>
            </div>

                <div className="mt-auto flex flex-col py-2 border-t border-[var(--border-color)]/30">
                <div className="px-4 pb-1 text-[11px] font-bold text-[var(--text-secondary)] tracking-wider uppercase opacity-60">TOOLS</div>
                <button
                    className={`sidebar-item flex items-center gap-2 ${currentView === 'activity' ? 'active' : ''}`}
                    onClick={() => onChangeView('activity')}
                >
                    <ClockIcon className="w-4 h-4" />
                    <span>Activity</span>
                </button>
                <button
                    className={`sidebar-item flex items-center gap-2 mt-1 ${currentView === 'settings' ? 'active' : ''}`}
                    onClick={() => onChangeView('settings')}
                >
                    <Cog6ToothIcon className="w-4 h-4" />
                    <span>Settings</span>
                </button>
                <a
                    href="https://linggen.dev/docs"
                    target="_blank"
                    rel="noopener noreferrer"
                    className="sidebar-item flex items-center gap-2 mt-1 no-underline"
                >
                    <QuestionMarkCircleIcon className="w-4 h-4" />
                    <span>Help</span>
                </a>
            </div>

            {contextMenu && (
                <ContextMenu
                    x={contextMenu.x}
                    y={contextMenu.y}
                    onClose={() => setContextMenu(null)}
                >
                    {contextMenu.libraryPackId ? (
                        <>
                            <ContextMenuItem
                                label="Rename"
                                icon={<PencilIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    const pack = libraryPacks.find(p => p.id === contextMenu.libraryPackId);
                                    if (pack) {
                                        setRenamingLibraryPack({ id: pack.id, oldName: pack.filename || pack.name });
                                    }
                                    setContextMenu(null);
                                }}
                            />
                            <ContextMenuItem
                                label="Delete"
                                icon={<TrashIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    const pack = libraryPacks.find(p => p.id === contextMenu.libraryPackId);
                                    if (pack) {
                                        setDeleteLibraryPackConfirmation({ id: pack.id, name: pack.filename || pack.name });
                                    }
                                    setContextMenu(null);
                                }}
                                danger={true}
                            />
                        </>
                    ) : contextMenu.libraryFolder ? (
                        <>
                            <ContextMenuItem
                                label="Add Doc"
                                icon={<DocumentPlusIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    setCreatingLibraryPack(contextMenu.libraryFolder!);
                                    setContextMenu(null);
                                }}
                            />
                            <ContextMenuItem
                                label="Add Folder"
                                icon={<FolderPlusIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    setCreatingLibraryFolder(contextMenu.libraryFolder!);
                                    if (!expandedLibraryFolders.has(contextMenu.libraryFolder!)) {
                                        toggleLibraryFolder(contextMenu.libraryFolder!);
                                    }
                                    setContextMenu(null);
                                }}
                            />
                            <ContextMenuItem
                                label="Rename"
                                icon={<PencilIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    setRenamingLibraryFolder({ oldName: contextMenu.libraryFolder! });
                                    setContextMenu(null);
                                }}
                            />
                            <ContextMenuItem
                                label="Delete"
                                icon={<TrashIcon className="w-3.5 h-3.5" />}
                                onClick={() => {
                                    setDeleteLibraryFolderConfirmation({ name: contextMenu.libraryFolder! });
                                    setContextMenu(null);
                                }}
                                danger={true}
                            />
                        </>
                    ) : (
                        <>
                            <ContextMenuItem
                                label="Add Doc"
                                icon={<DocumentPlusIcon className="w-3.5 h-3.5" />}
                                onClick={handleAddMarkdown}
                            />
                            {contextMenu.notePath ? (
                                <>
                                    <ContextMenuItem
                                        label="Rename"
                                        icon={<PencilIcon className="w-3.5 h-3.5" />}
                                        onClick={handleRenameNoteTrigger}
                                    />
                                    <ContextMenuItem
                                        label="Delete"
                                        icon={<TrashIcon className="w-3.5 h-3.5" />}
                                        onClick={handleDeleteNote}
                                        danger={true}
                                    />
                                </>
                            ) : (
                                <ContextMenuItem
                                    label="Remove Project"
                                    icon={<TrashIcon className="w-3.5 h-3.5" />}
                                    onClick={handleRemoveSource}
                                    danger={true}
                                />
                            )}
                        </>
                    )}
                </ContextMenu>
            )}

            {/* Delete Note Confirmation Modal */}
            {deleteConfirmation && (
                <div
                    className="fixed inset-0 bg-black/50 flex items-center justify-center z-[9999] pointer-events-auto"
                    onClick={() => setDeleteConfirmation(null)}
                >
                    <div
                        className="bg-[var(--bg-content)] border border-[var(--border-color)] rounded-lg p-6 w-[320px] shadow-xl flex flex-col gap-4"
                        onClick={(e) => e.stopPropagation()}
                    >
                        <h3 className="m-0 text-[1.1rem] font-semibold text-[var(--text-primary)]">Delete Note?</h3>
                        <p className="m-0 text-sm text-[var(--text-secondary)]">
                            Are you sure you want to delete <strong>{deleteConfirmation.notePath}</strong>? This action cannot be undone.
                        </p>
                        <div className="flex justify-end gap-3 mt-2">
                            <button
                                className="btn-secondary"
                                onClick={() => setDeleteConfirmation(null)}
                            >
                                Cancel
                            </button>
                            <button
                                className="btn-danger"
                                onClick={handleConfirmDelete}
                            >
                                Delete
                            </button>
                        </div>
                    </div>
                </div>
            )}

            {/* Remove Source Confirmation Modal */}
            {deleteSourceConfirmation && (
                <div
                    className="fixed inset-0 bg-black/50 flex items-center justify-center z-[9999] pointer-events-auto"
                    onClick={() => setDeleteSourceConfirmation(null)}
                >
                    <div
                        className="bg-[var(--bg-content)] border border-[var(--border-color)] rounded-lg p-6 w-[400px] shadow-xl flex flex-col gap-4"
                        onClick={(e) => e.stopPropagation()}
                    >
                        <h3 className="m-0 text-[1.1rem] font-semibold text-red-500">⚠️ Remove Source?</h3>
                        <div className="m-0 text-sm text-[var(--text-secondary)] flex flex-col gap-3">
                            <p>
                                Are you sure you want to remove <strong>{deleteSourceConfirmation.sourceName}</strong>?
                            </p>
                            <div>
                                <p className="text-[0.85rem] mb-2 font-semibold">This will permanently delete:</p>
                                <ul className="ml-5 list-disc text-[0.85rem] leading-relaxed">
                                    <li>All indexed files and chunks</li>
                                    <li>All vector embeddings (LanceDB)</li>
                                    <li>All metadata (redb)</li>
                                    <li>All notes and documents</li>
                                    <li>Graph cache</li>
                                </ul>
                            </div>
                            <p className="text-[0.85rem] text-red-500 font-semibold">
                                ⚠️ This action cannot be undone!
                            </p>
                        </div>
                        <div className="flex justify-end gap-3 mt-2">
                            <button
                                className="btn-secondary"
                                onClick={() => setDeleteSourceConfirmation(null)}
                            >
                                Cancel
                            </button>
                            <button
                                className="btn-danger"
                                onClick={handleConfirmRemoveSource}
                            >
                                Remove Source
                            </button>
                        </div>
                    </div>
                </div>
            )}

            {/* Delete Library Pack Confirmation Modal */}
            {deleteLibraryPackConfirmation && (
                <div
                    className="fixed inset-0 bg-black/50 flex items-center justify-center z-[9999] pointer-events-auto"
                    onClick={() => setDeleteLibraryPackConfirmation(null)}
                >
                    <div
                        className="bg-[var(--bg-content)] border border-[var(--border-color)] rounded-lg p-6 w-[320px] shadow-xl flex flex-col gap-4"
                        onClick={(e) => e.stopPropagation()}
                    >
                        <h3 className="m-0 text-[1.1rem] font-semibold text-[var(--text-primary)]">Delete Library Pack?</h3>
                        <p className="m-0 text-sm text-[var(--text-secondary)]">
                            Are you sure you want to delete <strong>{deleteLibraryPackConfirmation.name}</strong>? This action cannot be undone.
                        </p>
                        <div className="flex justify-end gap-3 mt-2">
                            <button
                                className="btn-secondary"
                                onClick={() => setDeleteLibraryPackConfirmation(null)}
                            >
                                Cancel
                            </button>
                            <button
                                className="btn-danger"
                                onClick={handleConfirmDeleteLibraryPack}
                            >
                                Delete
                            </button>
                        </div>
                    </div>
                </div>
            )}

            {/* Delete Library Folder Confirmation Modal */}
            {deleteLibraryFolderConfirmation && (
                <div
                    className="fixed inset-0 bg-black/50 flex items-center justify-center z-[9999] pointer-events-auto"
                    onClick={() => setDeleteLibraryFolderConfirmation(null)}
                >
                    <div
                        className="bg-[var(--bg-content)] border border-[var(--border-color)] rounded-lg p-6 w-[320px] shadow-xl flex flex-col gap-4"
                        onClick={(e) => e.stopPropagation()}
                    >
                        <h3 className="m-0 text-[1.1rem] font-semibold text-[var(--text-primary)]">Delete Library Folder?</h3>
                        <p className="m-0 text-sm text-[var(--text-secondary)]">
                            Are you sure you want to delete folder <strong>{deleteLibraryFolderConfirmation.name}</strong> and all its contents? This action cannot be undone.
                        </p>
                        <div className="flex justify-end gap-3 mt-2">
                            <button
                                className="btn-secondary"
                                onClick={() => setDeleteLibraryFolderConfirmation(null)}
                            >
                                Cancel
                            </button>
                            <button
                                className="btn-danger"
                                onClick={handleConfirmDeleteLibraryFolder}
                            >
                                Delete
                            </button>
                        </div>
                    </div>
                </div>
            )}
        </div>
    )
}
