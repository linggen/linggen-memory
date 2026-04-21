declare module 'react-force-graph-2d' {
  import { Component } from 'react';

  export interface NodeObject {
    id?: string | number;
    x?: number;
    y?: number;
    vx?: number;
    vy?: number;
    fx?: number;
    fy?: number;
    [key: string]: unknown;
  }

  export interface LinkObject {
    source?: string | number | NodeObject;
    target?: string | number | NodeObject;
    [key: string]: unknown;
  }

  export interface GraphData<N extends NodeObject = NodeObject, L extends LinkObject = LinkObject> {
    nodes: N[];
    links: L[];
  }

  export interface ForceGraphMethods<N extends NodeObject = NodeObject, L extends LinkObject = LinkObject> {
    // Camera controls
    centerAt(x?: number, y?: number, ms?: number): ForceGraphMethods<N, L>;
    zoom(k?: number, ms?: number): ForceGraphMethods<N, L>;
    zoomToFit(ms?: number, px?: number, nodeFilter?: (node: N) => boolean): ForceGraphMethods<N, L>;

    // Force engine
    d3Force(forceName: string): unknown;
    d3Force(forceName: string, force: unknown): ForceGraphMethods<N, L>;
    d3ReheatSimulation(): ForceGraphMethods<N, L>;
    
    // Utility
    screen2GraphCoords(x: number, y: number): { x: number; y: number };
    graph2ScreenCoords(x: number, y: number): { x: number; y: number };

    // Render
    pauseAnimation(): ForceGraphMethods<N, L>;
    resumeAnimation(): ForceGraphMethods<N, L>;
    refresh(): ForceGraphMethods<N, L>;
  }

  export interface ForceGraph2DProps<N extends NodeObject = NodeObject, L extends LinkObject = LinkObject> {
    // Data
    graphData?: GraphData<N, L>;

    // Container
    width?: number;
    height?: number;
    backgroundColor?: string;

    // Node
    nodeId?: string;
    nodeLabel?: string | ((node: N) => string);
    nodeVal?: number | string | ((node: N) => number);
    nodeRelSize?: number;
    nodeColor?: string | ((node: N) => string);
    nodeAutoColorBy?: string | ((node: N) => string | null);
    nodeCanvasObject?: (node: N, ctx: CanvasRenderingContext2D, globalScale: number) => void;
    nodeCanvasObjectMode?: string | ((node: N) => string);
    nodePointerAreaPaint?: (node: N, color: string, ctx: CanvasRenderingContext2D, globalScale: number) => void;

    // Link
    linkSource?: string;
    linkTarget?: string;
    linkLabel?: string | ((link: L) => string);
    linkVisibility?: boolean | string | ((link: L) => boolean);
    linkColor?: string | ((link: L) => string);
    linkAutoColorBy?: string | ((link: L) => string | null);
    linkLineDash?: number[] | string | ((link: L) => number[] | null);
    linkWidth?: number | string | ((link: L) => number);
    linkCurvature?: number | string | ((link: L) => number);
    linkCanvasObject?: (link: L, ctx: CanvasRenderingContext2D, globalScale: number) => void;
    linkCanvasObjectMode?: string | ((link: L) => string);
    linkDirectionalArrowLength?: number | string | ((link: L) => number);
    linkDirectionalArrowColor?: string | ((link: L) => string);
    linkDirectionalArrowRelPos?: number | string | ((link: L) => number);
    linkDirectionalParticles?: number | string | ((link: L) => number);
    linkDirectionalParticleSpeed?: number | string | ((link: L) => number);
    linkDirectionalParticleWidth?: number | string | ((link: L) => number);
    linkDirectionalParticleColor?: string | ((link: L) => string);
    linkPointerAreaPaint?: (link: L, color: string, ctx: CanvasRenderingContext2D, globalScale: number) => void;

    // Render control
    autoPauseRedraw?: boolean;
    minZoom?: number;
    maxZoom?: number;
    onRenderFramePre?: (ctx: CanvasRenderingContext2D, globalScale: number) => void;
    onRenderFramePost?: (ctx: CanvasRenderingContext2D, globalScale: number) => void;

    // Interaction
    onNodeClick?: (node: N, event: MouseEvent) => void;
    onNodeRightClick?: (node: N, event: MouseEvent) => void;
    onNodeHover?: (node: N | null, previousNode: N | null) => void;
    onNodeDrag?: (node: N, translate: { x: number; y: number }) => void;
    onNodeDragEnd?: (node: N, translate: { x: number; y: number }) => void;
    onLinkClick?: (link: L, event: MouseEvent) => void;
    onLinkRightClick?: (link: L, event: MouseEvent) => void;
    onLinkHover?: (link: L | null, previousLink: L | null) => void;
    onBackgroundClick?: (event: MouseEvent) => void;
    onBackgroundRightClick?: (event: MouseEvent) => void;
    onZoom?: (transform: { k: number; x: number; y: number }) => void;
    onZoomEnd?: (transform: { k: number; x: number; y: number }) => void;
    onEngineStop?: () => void;
    onEngineTick?: () => void;
    enableNodeDrag?: boolean;
    enableZoomInteraction?: boolean;
    enablePanInteraction?: boolean;
    enablePointerInteraction?: boolean;

    // Force engine
    dagMode?: string | null;
    dagLevelDistance?: number | null;
    dagNodeFilter?: (node: N) => boolean;
    onDagError?: (loopNodeIds: (string | number)[]) => void;
    d3AlphaMin?: number;
    d3AlphaDecay?: number;
    d3VelocityDecay?: number;
    ngraphPhysics?: object;
    warmupTicks?: number;
    cooldownTicks?: number;
    cooldownTime?: number;
  }

  export default class ForceGraph2D<
    N extends NodeObject = NodeObject,
    L extends LinkObject = LinkObject
  > extends Component<ForceGraph2DProps<N, L>> {
    // Methods exposed via ref
    centerAt: ForceGraphMethods<N, L>['centerAt'];
    zoom: ForceGraphMethods<N, L>['zoom'];
    zoomToFit: ForceGraphMethods<N, L>['zoomToFit'];
    d3Force: ForceGraphMethods<N, L>['d3Force'];
    d3ReheatSimulation: ForceGraphMethods<N, L>['d3ReheatSimulation'];
    screen2GraphCoords: ForceGraphMethods<N, L>['screen2GraphCoords'];
    graph2ScreenCoords: ForceGraphMethods<N, L>['graph2ScreenCoords'];
    pauseAnimation: ForceGraphMethods<N, L>['pauseAnimation'];
    resumeAnimation: ForceGraphMethods<N, L>['resumeAnimation'];
    refresh: ForceGraphMethods<N, L>['refresh'];
  }
}
