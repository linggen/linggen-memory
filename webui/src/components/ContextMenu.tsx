import React, { useEffect, useRef } from 'react';

interface ContextMenuProps {
    x: number;
    y: number;
    onClose: () => void;
    children: React.ReactNode;
}

export const ContextMenu: React.FC<ContextMenuProps> = ({ x, y, onClose, children }) => {
    const menuRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const handleClickOutside = (event: MouseEvent) => {
            if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
                onClose();
            }
        };

        const handleEscape = (event: KeyboardEvent) => {
            if (event.key === 'Escape') {
                onClose();
            }
        };

        const handleRightClick = (e: MouseEvent) => {
            // Close if right-clicking elsewhere, let parent handle reopening
            if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
                onClose();
            }
        };

        document.addEventListener('mousedown', handleClickOutside);
        document.addEventListener('keydown', handleEscape);
        document.addEventListener('contextmenu', handleRightClick);

        return () => {
            document.removeEventListener('mousedown', handleClickOutside);
            document.removeEventListener('keydown', handleEscape);
            document.removeEventListener('contextmenu', handleRightClick);
        };
    }, [onClose]);

    // Adjust position if it spills off-screen (basic version)
    // We can enhance this to measure window size

    return (
        <div
            ref={menuRef}
            className="fixed z-[1000] bg-[var(--bg-content)] border border-[var(--border-color)] rounded-md shadow-lg py-1 min-w-[160px] flex flex-col"
            style={{
                top: y,
                left: x,
            }}
            onClick={(e) => e.stopPropagation()}
            onContextMenu={(e) => e.preventDefault()}
        >
            {children}
        </div>
    );
};

interface ContextMenuItemProps {
    onClick: () => void;
    label: string;
    icon?: React.ReactNode;
    color?: string;
    danger?: boolean;
}

export const ContextMenuItem: React.FC<ContextMenuItemProps> = ({ onClick, label, icon, color, danger }) => {
    return (
        <button
            className={`flex items-center gap-2 px-3 py-2 w-full border-none bg-transparent text-left text-[0.85rem] cursor-pointer hover:bg-[var(--bg-secondary)] transition-colors ${danger ? 'text-red-500' : (color || 'text-[var(--text-primary)]')}`}
            onClick={(e) => {
                e.stopPropagation(); // Prevent Sidebar click
                onClick();
            }}
        >
            {icon && <span className="w-4 h-4 flex items-center justify-center">{icon}</span>}
            <span>{label}</span>
        </button>
    );
};
