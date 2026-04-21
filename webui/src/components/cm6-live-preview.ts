/**
 * Live Preview Plugin for CodeMirror 6
 * 
 * This plugin hides markdown syntax and shows rendered content inline,
 * similar to Obsidian's Live Preview mode.
 * 
 * When cursor is on a line, syntax is shown; when cursor moves away, 
 * the markdown is rendered.
 */

import {
    Decoration,
    type DecorationSet,
    EditorView,
    ViewPlugin,
    type ViewUpdate,
    WidgetType
} from '@codemirror/view';
import { syntaxTree } from '@codemirror/language';
import { RangeSetBuilder } from '@codemirror/state';

// Lazy mermaid import to avoid blocking
let mermaidInstance: typeof import('mermaid').default | null = null;
let mermaidInitialized = false;

async function getMermaid() {
    if (!mermaidInstance) {
        try {
            const mermaidModule = await import('mermaid');
            mermaidInstance = mermaidModule.default;
            if (!mermaidInitialized) {
                const rootTheme = document.documentElement.getAttribute('data-theme');
                const isLight = rootTheme === 'light' || (!rootTheme && window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches);
                mermaidInstance.initialize({
                    startOnLoad: false,
                    theme: isLight ? 'default' : 'dark',
                    securityLevel: 'loose',
                });
                mermaidInitialized = true;
            }
        } catch (err) {
            console.error('Failed to load mermaid:', err);
            return null;
        }
    }
    return mermaidInstance;
}

// === Widget Classes for replaced content ===
//
// Track which mermaid blocks are explicitly in "edit raw text" mode.
// Keyed by the starting position of the fenced block.
const mermaidEditBlocks = new Set<number>();

class MermaidWidget extends WidgetType {
    private code: string;
    private id: string;
    private blockPos: number; // Position in document for tracking edit state

    constructor(code: string, blockPos: number) {
        super();
        this.code = code;
        this.blockPos = blockPos;
        this.id = `mermaid-${Math.random().toString(36).substr(2, 9)}`;
    }

    toDOM(view: EditorView) {
        const container = document.createElement('div');
        container.className = 'cm-mermaid-container relative my-2';

        // Edit button overlay
        const editBtn = document.createElement('button');
        editBtn.className = 'cm-mermaid-edit-btn absolute top-2 right-2 bg-[#646cff]/80 border-none text-white px-2 py-1 rounded cursor-pointer text-[12px] z-10 opacity-0 transition-opacity duration-200';
        editBtn.innerHTML = '&lt;/&gt;';
        editBtn.title = 'Edit this block';

        // Show button on hover
        container.addEventListener('mouseenter', () => {
            editBtn.classList.remove('opacity-0');
            editBtn.classList.add('opacity-100');
        });
        container.addEventListener('mouseleave', () => {
            editBtn.classList.remove('opacity-100');
            editBtn.classList.add('opacity-0');
        });

        // Click to focus on the mermaid code block in the editor
        editBtn.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            // Mark this block as explicitly being edited in raw mode.
            mermaidEditBlocks.add(this.blockPos);
            // Move cursor to the start of this block to make editing natural.
            view.dispatch({
                selection: { anchor: this.blockPos },
                scrollIntoView: true
            });
            view.focus();
        });

        // Diagram container (no special pointer/scroll handling; let CM6 handle it)
        const diagramContainer = document.createElement('div');
        diagramContainer.className = 'cm-mermaid-diagram flex justify-center p-4 bg-black/15 rounded-lg';
        diagramContainer.innerHTML = '<div class="text-[#94a3b8] p-4">Loading diagram...</div>';

        container.appendChild(editBtn);
        container.appendChild(diagramContainer);

        // Render mermaid asynchronously
        this.renderMermaid(diagramContainer);

        return container;
    }

    private async renderMermaid(container: HTMLElement) {
        try {
            const mermaid = await getMermaid();
            if (!mermaid) {
                container.innerHTML = '<div class="text-red-500 p-2">Mermaid not available</div>';
                return;
            }
            const cleanCode = this.code.trim();
            const { svg } = await mermaid.render(this.id, cleanCode);
            container.innerHTML = svg;
        } catch (err) {
            container.innerHTML = `<div class="text-red-500 p-2">Mermaid Error: ${err instanceof Error ? err.message : String(err)}</div>`;
        }
    }

    eq(other: MermaidWidget) {
        return this.code === other.code && this.blockPos === other.blockPos;
    }

    // Let CodeMirror handle events normally. We rely on explicit state
    // (`mermaidEditBlocks`) to decide raw vs preview, so clicks on the
    // diagram no longer force a mode change.
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ignoreEvent(_event: Event) {
        return false;
    }
}

// === Widget Classes for replaced content ===

class HorizontalRuleWidget extends WidgetType {
    toDOM() {
        const hr = document.createElement('hr');
        hr.className = 'cm-hr-widget';
        return hr;
    }
}

class CheckboxWidget extends WidgetType {
    private checked: boolean;

    constructor(checked: boolean) {
        super();
        this.checked = checked;
    }

    toDOM() {
        const span = document.createElement('span');
        span.className = `cm-checkbox-widget ${this.checked ? 'checked' : ''}`;
        span.textContent = this.checked ? '☑' : '☐';
        return span;
    }
}

class BulletWidget extends WidgetType {
    toDOM() {
        const span = document.createElement('span');
        span.className = 'cm-list-bullet';
        span.textContent = '• ';
        return span;
    }
}

// === Decoration Classes ===

const hiddenMarkDecoration = Decoration.mark({ class: 'cm-hidden-syntax' });
const boldDecoration = Decoration.mark({ class: 'cm-rendered-strong' });
const italicDecoration = Decoration.mark({ class: 'cm-rendered-emphasis' });
const strikeDecoration = Decoration.mark({ class: 'cm-rendered-strike' });
const linkDecoration = Decoration.mark({ class: 'cm-rendered-link' });
const codeDecoration = Decoration.mark({ class: 'cm-rendered-code' });
const blockquoteDecoration = Decoration.line({ class: 'cm-blockquote-line' });

// === Helper functions ===

function getActiveLines(view: EditorView): Set<number> {
    const activeLines = new Set<number>();
    for (const range of view.state.selection.ranges) {
        const startLine = view.state.doc.lineAt(range.from).number;
        const endLine = view.state.doc.lineAt(range.to).number;
        for (let i = startLine; i <= endLine; i++) {
            activeLines.add(i);
        }
    }
    return activeLines;
}

// === The main ViewPlugin ===

export const livePreviewPlugin = ViewPlugin.fromClass(
    class {
        decorations: DecorationSet;

        constructor(view: EditorView) {
            this.decorations = this.buildDecorations(view);
        }

        update(update: ViewUpdate) {
            if (update.docChanged || update.selectionSet || update.viewportChanged) {
                this.decorations = this.buildDecorations(update.view);
            }
        }

        buildDecorations(view: EditorView): DecorationSet {
            const activeLines = getActiveLines(view);
            const doc = view.state.doc;

            // Collect all decorations first (we need to sort them)
            const decorations: { from: number; to: number; decoration: Decoration }[] = [];

            // === Standard markdown live preview decorations ===
            for (const { from, to } of view.visibleRanges) {
                syntaxTree(view.state).iterate({
                    from,
                    to,
                    enter: (node) => {
                        const line = doc.lineAt(node.from);
                        const isActiveLine = activeLines.has(line.number);

                        // Skip decorations on active lines (show raw markdown while editing)
                        if (isActiveLine) return;

                        const nodeType = node.name;

                        // Headers - hide # marks
                        if (nodeType.startsWith('ATXHeading') || nodeType === 'HeaderMark') {
                            if (nodeType === 'HeaderMark') {
                                // Hide the # marks directly
                                decorations.push({
                                    from: node.from,
                                    to: node.to + 1, // +1 for the space after
                                    decoration: hiddenMarkDecoration
                                });
                            }
                        }

                        // Bold **text** or __text__ - look for EmphasisMark
                        if (nodeType === 'StrongEmphasis') {
                            const text = doc.sliceString(node.from, node.to);
                            const marker = text.startsWith('**') ? '**' : '__';
                            decorations.push({ from: node.from, to: node.from + marker.length, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.to - marker.length, to: node.to, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.from + marker.length, to: node.to - marker.length, decoration: boldDecoration });
                        }

                        // Italic *text* or _text_
                        if (nodeType === 'Emphasis') {
                            const text = doc.sliceString(node.from, node.to);
                            const marker = text.startsWith('*') ? '*' : '_';
                            decorations.push({ from: node.from, to: node.from + marker.length, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.to - marker.length, to: node.to, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.from + marker.length, to: node.to - marker.length, decoration: italicDecoration });
                        }

                        // Strikethrough ~~text~~
                        if (nodeType === 'Strikethrough') {
                            decorations.push({ from: node.from, to: node.from + 2, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.to - 2, to: node.to, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.from + 2, to: node.to - 2, decoration: strikeDecoration });
                        }

                        // Inline code `code` - look for InlineCode or CodeMark
                        if (nodeType === 'InlineCode') {
                            decorations.push({ from: node.from, to: node.from + 1, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.to - 1, to: node.to, decoration: hiddenMarkDecoration });
                            decorations.push({ from: node.from + 1, to: node.to - 1, decoration: codeDecoration });
                        }

                        // Links [text](url)
                        if (nodeType === 'Link') {
                            const text = doc.sliceString(node.from, node.to);
                            const linkMatch = text.match(/^\[([^\]]*)\]\(([^)]*)\)$/);
                            if (linkMatch) {
                                const textStart = node.from + 1;
                                const textEnd = node.from + 1 + linkMatch[1].length;
                                decorations.push({ from: node.from, to: node.from + 1, decoration: hiddenMarkDecoration });
                                decorations.push({ from: textEnd, to: node.to, decoration: hiddenMarkDecoration });
                                decorations.push({ from: textStart, to: textEnd, decoration: linkDecoration });
                            }
                        }

                        // Blockquotes > - look for QuoteMark
                        if (nodeType === 'QuoteMark') {
                            decorations.push({ from: node.from, to: node.to + 1, decoration: hiddenMarkDecoration });
                        }

                        if (nodeType === 'Blockquote') {
                            decorations.push({ from: line.from, to: line.from, decoration: blockquoteDecoration });
                        }

                        // Horizontal rule ---
                        if (nodeType === 'HorizontalRule') {
                            decorations.push({
                                from: node.from,
                                to: node.to,
                                decoration: Decoration.replace({ widget: new HorizontalRuleWidget() })
                            });
                        }

                        // List markers
                        if (nodeType === 'ListMark') {
                            decorations.push({
                                from: node.from,
                                to: node.to + 1,
                                decoration: Decoration.replace({ widget: new BulletWidget() })
                            });
                        }

                        // Task list checkboxes
                        if (nodeType === 'TaskMarker') {
                            const text = doc.sliceString(node.from, node.to);
                            const isChecked = text.includes('x') || text.includes('X');
                            decorations.push({
                                from: node.from,
                                to: node.to,
                                decoration: Decoration.replace({ widget: new CheckboxWidget(isChecked) })
                            });
                        }
                    }
                });
            }

            // === Mermaid fenced code blocks ===
            const fullText = doc.toString();
            const mermaidRegex = /```mermaid\s*\n([\s\S]*?)```/g;
            let match: RegExpExecArray | null;

            while ((match = mermaidRegex.exec(fullText)) !== null) {
                const blockCode = match[1];
                const blockStart = match.index;
                const blockEnd = match.index + match[0].length;

                // If this block is marked as "edit raw", keep it in raw mode
                // until the selection moves completely outside the block.
                if (mermaidEditBlocks.has(blockStart)) {
                    let stillEditing = false;
                    for (const range of view.state.selection.ranges) {
                        if (range.from <= blockEnd && range.to >= blockStart) {
                            stillEditing = true;
                            break;
                        }
                    }
                    if (!stillEditing) {
                        // Cursor moved out of the block -> clear edit mode.
                        mermaidEditBlocks.delete(blockStart);
                    } else {
                        // Stay in raw mode for this block.
                        continue;
                    }
                }

                // Preview mode: render the diagram and hide the fenced code.
                decorations.push({
                    from: blockStart,
                    to: blockStart,
                    decoration: Decoration.widget({
                        widget: new MermaidWidget(blockCode, blockStart),
                        // Block widgets are not allowed from view plugins in CM6,
                        // so we keep this inline but style it as a block-level
                        // container via CSS.
                    })
                });

                // Hide the original fenced code lines (but keep layout)
                const startLine = doc.lineAt(blockStart).number;
                const endLine = doc.lineAt(blockEnd).number;
                for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
                    const line = doc.line(lineNo);
                    decorations.push({
                        from: line.from,
                        to: line.from,
                        decoration: Decoration.line({ class: 'cm-mermaid-hidden-line' })
                    });
                }
            }

            // Sort decorations by 'from' position, then by 'to' position
            decorations.sort((a, b) => a.from - b.from || a.to - b.to);

            // Build the decoration set
            const builder = new RangeSetBuilder<Decoration>();
            for (const { from, to, decoration } of decorations) {
                try {
                    builder.add(from, to, decoration);
                } catch {
                    // Skip invalid ranges
                }
            }

            return builder.finish();
        }
    },
    {
        decorations: (v) => v.decorations,
    }
);

// === Theme for live preview elements ===

export const livePreviewTheme = EditorView.theme({
    // Hidden syntax
    '.cm-hidden-syntax': {
        fontSize: '0',
        width: '0',
        display: 'none',
    },

    // Headers
    '.cm-header-line.cm-header-1': {
        fontSize: '1.8em',
        fontWeight: 'bold',
        lineHeight: '1.3',
        color: 'var(--text-active)',
    },
    '.cm-header-line.cm-header-2': {
        fontSize: '1.5em',
        fontWeight: 'bold',
        lineHeight: '1.3',
        color: 'var(--text-active)',
    },
    '.cm-header-line.cm-header-3': {
        fontSize: '1.3em',
        fontWeight: 'bold',
        lineHeight: '1.3',
        color: 'var(--text-active)',
    },

    // Bold
    '.cm-rendered-strong': {
        fontWeight: 'bold',
        color: 'var(--text-active)',
    },

    // Italic
    '.cm-rendered-emphasis': {
        fontStyle: 'italic',
        color: 'var(--text-secondary)',
    },

    // Strikethrough
    '.cm-rendered-strike': {
        textDecoration: 'line-through',
        color: 'var(--text-muted)',
    },

    // Links
    '.cm-rendered-link': {
        color: 'var(--accent)',
        textDecoration: 'underline',
        cursor: 'pointer',
    },

    // Inline code
    '.cm-rendered-code': {
        backgroundColor: 'color-mix(in srgb, var(--accent), transparent 85%)',
        color: 'var(--accent)',
        padding: '2px 6px',
        borderRadius: '4px',
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
        fontSize: '0.9em',
    },

    // Blockquotes
    '.cm-blockquote-line': {
        borderLeft: '3px solid var(--accent)',
        paddingLeft: '12px',
        color: 'var(--text-muted)',
        fontStyle: 'italic',
    },

    // List bullets
    '.cm-list-bullet': {
        color: 'var(--accent)',
        fontWeight: 'bold',
        marginRight: '8px',
    },

    // Checkboxes
    '.cm-checkbox-widget': {
        display: 'inline-block',
        width: '18px',
        height: '18px',
        marginRight: '8px',
        fontSize: '16px',
        color: 'var(--text-muted)',
        cursor: 'pointer',
    },
    '.cm-checkbox-widget.checked': {
        color: '#22c55e',
    },

    // Horizontal rule
    '.cm-hr-widget': {
        display: 'block',
        border: 'none',
        borderTop: '1px solid var(--border-color)',
        margin: '16px 0',
    },

    // Hidden mermaid code lines (collapsed but not display:none to preserve CM6 layout)
    '.cm-mermaid-hidden-line': {
        height: '0 !important',
        padding: '0 !important',
        margin: '0 !important',
        overflow: 'hidden !important',
        opacity: '0',
        fontSize: '0',
        lineHeight: '0',
    },

    // Mermaid diagram container
    '.cm-mermaid-widget': {
        display: 'flex',
        justifyContent: 'center',
        padding: '16px',
        background: 'var(--bg-app)',
        borderRadius: '8px',
        margin: '8px 0',
        minHeight: '100px',
    },
});
