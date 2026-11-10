// What every chart tab of the node window gets from the window: the range, the
// current time and the window-wide zoom (drag on any chart pauses all of them).
export interface ChartProps {
  rangeS: number;
  /** The latest snapshot's time, unix seconds. */
  nowS: number;
  zoom: [number, number] | null;
  onZoom: (z: [number, number] | null) => void;
}
