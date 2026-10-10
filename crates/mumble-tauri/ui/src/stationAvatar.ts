/**
 * Station avatar texture (Option B).
 *
 * A Station publishes this antenna image as its Mumble user texture on
 * connect, so EVERY client — RemoteRadio or stock Mumble, in any channel —
 * sees the station marker as the user's avatar. Because a client can only set
 * its own texture, an operator cannot override a station's icon.
 *
 * The PNG is imported as an inlined data URL so the bytes are available
 * synchronously without a fetch.
 */
import antennaDataUrl from "./assets/station-antenna.png?inline";
import { dataUrlToBytes } from "./profileFormat";

let cachedBytes: number[] | null = null;

/** The antenna texture as a Mumble texture byte array (PNG). */
export function stationAvatarBytes(): number[] {
  if (cachedBytes === null) {
    cachedBytes = dataUrlToBytes(antennaDataUrl);
  }
  return cachedBytes;
}
