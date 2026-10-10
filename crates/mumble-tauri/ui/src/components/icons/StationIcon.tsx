import { Antenna } from "lucide-react";
import type { ComponentProps } from "react";

/**
 * Station identity glyph.
 *
 * This is the SAME antenna icon shown next to "Station" in the connect-page
 * role toggle (lucide `Antenna`): a mast with a small dipole "V" and a signal
 * arc. Centralised here as `StationIcon` so every place a Remote.Radio Station
 * is shown uses one consistent glyph; swapping the underlying icon is a
 * one-line change.
 */
export function StationIcon(props: Readonly<ComponentProps<typeof Antenna>>) {
  return <Antenna {...props} />;
}

export default StationIcon;
