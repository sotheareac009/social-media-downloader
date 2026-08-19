import { invoke } from "@tauri-apps/api/core";

export interface NetStatus {
  online: boolean;
  /** TCP round-trip in ms, when online. */
  ms: number | null;
  /** Host that answered, e.g. "1.1.1.1". */
  host: string | null;
}

/** Probe internet connectivity + latency from the Rust backend. */
export const netPing = () => invoke<NetStatus>("net_ping");
