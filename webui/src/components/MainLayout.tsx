import type { ReactNode } from 'react'
import { Sidebar, type View } from './Sidebar'
import type { Resource, LibraryPack } from '../api'

// Helper function to format bytes into human-readable size
function formatSize(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

interface MainLayoutProps {
    currentView: View
    onChangeView: (view: View) => void
    statusElement: ReactNode
    children: ReactNode
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

export function MainLayout({
    currentView,
    onChangeView,
    statusElement,
    children,
    resources,
    resourcesVersion,
    selectedSourceId,
    onSelectSource,
    selectedNotePath,
    onSelectNote,
    selectedMemoryPath,
    onSelectMemory,
    selectedLibraryPackId,
    onSelectLibraryPack,
    onAddSource,
    libraryPacks,
    libraryFolders,
    onRefresh
}: MainLayoutProps) {
    // We can add state for collapsing sidebar later if needed
    // const [isSidebarOpen, setIsSidebarOpen] = useState(true)

    return (
        <div className="flex h-screen w-screen overflow-hidden">
            {/* Left Sidebar */}
            <div className="w-[260px] bg-[var(--bg-sidebar)] border-r border-[var(--border-color)] flex flex-col flex-shrink-0">
                <div className="h-[50px] flex items-center px-4 border-b border-[var(--border-color)] gap-2.5">
                    <div className="w-6 h-6 bg-[var(--accent)] text-white rounded flex items-center justify-center font-extrabold text-[11px]">LG</div>
                    <div className="font-semibold text-[var(--text-active)] text-[13px] tracking-wider">Linggen</div>
                </div>
                <Sidebar
                    currentView={currentView}
                    onChangeView={onChangeView}
                    resources={resources}
                    resourcesVersion={resourcesVersion}
                    selectedSourceId={selectedSourceId}
                    onSelectSource={onSelectSource}
                    selectedNotePath={selectedNotePath}
                    onSelectNote={onSelectNote}
                    selectedMemoryPath={selectedMemoryPath}
                    onSelectMemory={onSelectMemory}
                    selectedLibraryPackId={selectedLibraryPackId}
                    onSelectLibraryPack={onSelectLibraryPack}
                    onAddSource={onAddSource}
                    libraryPacks={libraryPacks}
                    libraryFolders={libraryFolders}
                    onRefresh={onRefresh}
                />
            </div>

            <div className="flex-1 flex flex-col min-w-0 bg-[var(--bg-content)]">
                <header className="h-12 flex items-center px-6 border-b border-[var(--border-color)]">
                    {/* Breadcrumbs or Title could go here */}
                    <div className="text-sm font-semibold text-[var(--text-active)]">
                        {currentView === 'sources' ? 'Projects' : currentView.charAt(0).toUpperCase() + currentView.slice(1)}
                    </div>
                    {currentView === 'sources' && resources && resources.length > 0 && (
                        <div className="ml-auto flex items-center gap-3 text-[0.85rem] text-[var(--text-secondary)]">
                            <span>{resources.length} {resources.length === 1 ? 'project' : 'projects'}</span>
                            <span>•</span>
                            <span>
                                {formatSize(
                                    resources.reduce((total, r) => 
                                        total + (r.stats?.total_size_bytes || 0), 0
                                    )
                                )}
                            </span>
                        </div>
                    )}
                </header>
                <main className="flex-1 overflow-hidden flex flex-col">
                    {children}
                </main>
                {statusElement && (
                    <footer className="h-[22px] bg-[var(--bg-status-bar)] border-t border-[var(--border-color)] flex items-center px-2 text-[11px] text-[var(--text-secondary)]">
                        {statusElement}
                    </footer>
                )}
            </div>
        </div>
    )
}
