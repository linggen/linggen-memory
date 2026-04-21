import React, { useEffect, useState, useCallback, useRef } from 'react';
import CodeMirror, { type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { javascript } from '@codemirror/lang-javascript';
import { python } from '@codemirror/lang-python';
import { languages } from '@codemirror/language-data';
import { oneDark } from '@codemirror/theme-one-dark';
import { EditorView } from '@codemirror/view';
import { getNote, saveNote, getMemoryFile, saveMemoryFile, getPack, savePack } from '../api';
import { livePreviewPlugin, livePreviewTheme } from './cm6-live-preview';

interface CM6EditorProps {
    sourceId: string;
    docPath: string;
    docType?: 'notes' | 'memory' | 'library';
    /**
     * How scrolling should work:
     * - editor: CodeMirror scrolls internally (default, best for large docs)
     * - container: editor grows with content; parent container scrolls
     */
    scrollMode?: 'editor' | 'container';
    onClose?: () => void;
    readOnly?: boolean;
}

// Custom theme to match Linggen dark mode
const linggenBaseTheme = EditorView.theme({
    '&': {
        fontSize: '14px',
    },
    '.cm-content': {
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
        padding: '16px',
    },
    '.cm-line': {
        padding: '0 4px',
    },
    '.cm-gutters': {
        backgroundColor: 'transparent',
        borderRight: '1px solid var(--border-color)',
    },
    '.cm-activeLineGutter': {
        backgroundColor: 'var(--item-hover)',
        opacity: '0.5',
    },
    '.cm-activeLine': {
        backgroundColor: 'var(--item-hover)',
        opacity: '0.3',
    },
    '.cm-selectionBackground': {
        backgroundColor: 'var(--accent)',
        opacity: '0.3',
    },
    '.cm-cursor': {
        borderLeftColor: 'var(--text-primary)',
    },
    // Markdown-specific styling
    '.cm-header-1': {
        fontSize: '1.8em',
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },
    '.cm-header-2': {
        fontSize: '1.5em',
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },
    '.cm-header-3': {
        fontSize: '1.3em',
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },
    '.cm-header-4, .cm-header-5, .cm-header-6': {
        fontSize: '1.1em',
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },
    '.cm-strong': {
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },
    '.cm-emphasis': {
        fontStyle: 'italic',
        color: 'var(--text-secondary)',
    },
    '.cm-strikethrough': {
        textDecoration: 'line-through',
    },
    '.cm-link': {
        color: 'var(--accent)',
        textDecoration: 'underline',
    },
    '.cm-url': {
        color: 'var(--text-muted)',
    },
    '.cm-code': {
        backgroundColor: 'color-mix(in srgb, var(--accent), transparent 90%)',
        color: 'var(--accent)',
        padding: '2px 4px',
        borderRadius: '3px',
    },
});

const linggenFillHeightTheme = EditorView.theme(
    {
        '&': { height: '100%' },
        '.cm-scroller': { height: '100%' },
    }
);

const linggenAutoHeightTheme = EditorView.theme(
    {
        '&': { height: 'auto' },
        '.cm-scroller': { overflow: 'visible' },
    }
);

// Helper function to get language extension based on file path
function getLanguageExtension(docPath: string) {
    const ext = docPath.split('.').pop()?.toLowerCase();

    switch (ext) {
        case 'js':
        case 'jsx':
            return javascript({ jsx: true });
        case 'ts':
        case 'tsx':
            return javascript({ jsx: true, typescript: true });
        case 'py':
            return python();
        case 'sh':
        case 'bash':
        case 'zsh':
            // For shell scripts, use basic syntax highlighting
            // We can add a dedicated shell language later if needed
            return javascript(); // Fallback to JS for basic highlighting
        case 'md':
        default:
            return markdown({
                base: markdownLanguage,
                codeLanguages: languages
            });
    }
}

export const CM6Editor: React.FC<CM6EditorProps> = ({
    sourceId,
    docPath,
    docType = 'notes',
    scrollMode = 'editor',
    readOnly = false,
}) => {
    const [value, setValue] = useState<string>('');
    const [loading, setLoading] = useState(false);
    const [saving, setSaving] = useState(false);
    const [dirty, setDirty] = useState(false);
    const [lastSaved, setLastSaved] = useState<Date | null>(null);
    const editorRef = useRef<ReactCodeMirrorRef>(null);

    // Initial load
    useEffect(() => {
        const load = async () => {
            setLoading(true);
            try {
                const doc =
                    docType === 'memory'
                        ? await getMemoryFile(sourceId, docPath)
                        : docType === 'library'
                        ? await getPack(docPath)
                        : await getNote(sourceId, docPath);
                setValue(doc.content || '');
                setDirty(false);
            } catch (err) {
                console.error("Failed to load note:", err);
                setValue("# Error loading document\n\n" + String(err));
            } finally {
                setLoading(false);
            }
        };
        load();
    }, [sourceId, docPath, docType]);

    // Save handler
    const handleSave = useCallback(async () => {
        if (!dirty || readOnly) return;
        setSaving(true);
        try {
            if (docType === 'memory') {
                await saveMemoryFile(sourceId, docPath, value);
            } else if (docType === 'library') {
                await savePack(docPath, value);
            } else {
                await saveNote(sourceId, docPath, value);
            }
            setLastSaved(new Date());
            setDirty(false);
        } catch (err) {
            console.error("Failed to save note:", err);
        } finally {
            setSaving(false);
        }
    }, [dirty, sourceId, docPath, docType, value, readOnly]);

    // Autosave with debounce (1.5 seconds after typing stops)
    useEffect(() => {
        if (!dirty || readOnly) return;

        const timer = setTimeout(() => {
            handleSave();
        }, 1500);

        return () => clearTimeout(timer);
    }, [value, dirty, handleSave, readOnly]);

    // Cmd+S support
    useEffect(() => {
        if (readOnly) return;
        const handleKeyDown = (e: KeyboardEvent) => {
            if ((e.metaKey || e.ctrlKey) && e.key === 's') {
                e.preventDefault();
                handleSave();
            }
        };
        window.addEventListener('keydown', handleKeyDown);
        return () => window.removeEventListener('keydown', handleKeyDown);
    }, [handleSave, readOnly]);

    const handleChange = useCallback((val: string) => {
        if (readOnly) return;
        setValue(val);
        setDirty(true);
    }, [readOnly]);

    const [isDarkMode, setIsDarkMode] = useState(() => {
        const rootTheme = document.documentElement.getAttribute('data-theme');
        if (rootTheme === 'dark') return true;
        if (rootTheme === 'light') return false;
        return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
    });

    useEffect(() => {
        const observer = new MutationObserver((mutations) => {
            for (const mutation of mutations) {
                if (mutation.type === 'attributes' && mutation.attributeName === 'data-theme') {
                    const newTheme = document.documentElement.getAttribute('data-theme');
                    if (newTheme === 'dark') setIsDarkMode(true);
                    else if (newTheme === 'light') setIsDarkMode(false);
                    else setIsDarkMode(window.matchMedia('(prefers-color-scheme: dark)').matches);
                }
            }
        });

        observer.observe(document.documentElement, { attributes: true });

        const mq = window.matchMedia('(prefers-color-scheme: dark)');
        const handler = (e: MediaQueryListEvent) => {
            const currentTheme = document.documentElement.getAttribute('data-theme');
            if (!currentTheme || currentTheme === 'system') {
                setIsDarkMode(e.matches);
            }
        };
        mq.addEventListener('change', handler);

        return () => {
            observer.disconnect();
            mq.removeEventListener('change', handler);
        };
    }, []);

    if (loading) {
        return (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)] text-[0.9rem] bg-[var(--bg-content)]">
                Loading {docPath}...
            </div>
        );
    }

    const isContainerScroll = scrollMode === 'container';
    const isMarkdown = docPath.toLowerCase().endsWith('.md');

    return (
        <div className={`flex flex-col h-full min-h-0 bg-[var(--bg-content)] overflow-hidden ${isContainerScroll ? 'h-auto overflow-visible' : ''}`}>
            {/* Header / Status Bar */}
            <div className="flex justify-between items-center px-4 py-2 border-b border-[var(--border-color)] bg-[var(--bg-content)]">
                <div className="text-[0.9rem] font-medium text-[var(--text-primary)]">
                    {docType === 'memory' ? `Memory: ${docPath}` : docType === 'library' ? `Library: ${docPath}` : docPath}
                </div>
                <div className="text-[0.8rem] text-[var(--text-muted)]">
                    {readOnly ? (
                        <span className="bg-[var(--border-color)] px-2 py-0.5 rounded text-[10px] uppercase font-bold tracking-wider">Read Only</span>
                    ) : saving ? (
                        <span className="text-amber-500">Saving...</span>
                    ) : lastSaved ? (
                        <span className="text-green-500">Saved {lastSaved.toLocaleTimeString()}</span>
                    ) : dirty ? (
                        <span className="text-amber-500">Unsaved</span>
                    ) : null}
                </div>
            </div>

            {/* Editor */}
            <div className={`min-h-0 ${isContainerScroll ? 'block overflow-visible' : 'flex-1 overflow-hidden'}`}>
                <CodeMirror
                    ref={editorRef}
                    value={value}
                    onChange={handleChange}
                    readOnly={readOnly}
                    height={scrollMode === 'editor' ? '100%' : 'auto'}
                    minHeight="200px"
                    className={isContainerScroll ? 'cm-auto-height' : 'h-full'}
                    theme={isDarkMode ? oneDark : 'light'}
                    extensions={[
                        getLanguageExtension(docPath),
                        linggenBaseTheme,
                        scrollMode === 'editor' ? linggenFillHeightTheme : linggenAutoHeightTheme,
                        // Only include live preview for markdown files
                        ...(isMarkdown ? [livePreviewPlugin, livePreviewTheme] : []),
                        EditorView.lineWrapping,
                    ]}
                    basicSetup={{
                        lineNumbers: !isMarkdown, // Show line numbers for code files
                        foldGutter: true,
                        highlightActiveLineGutter: true,
                        highlightActiveLine: true,
                        bracketMatching: true,
                        closeBrackets: true,
                        autocompletion: true,
                        history: true,
                        searchKeymap: true,
                    }}
                />
            </div>
        </div>
    );
};
