import { invoke } from '@tauri-apps/api/core';

type Layout = { x: number; y: number; width: number; height: number; visible: boolean };
type TerminalUpdate =
  | [command: 'terminal_layout', arguments_: { identifier: number; layout: Layout }, expectedClose: () => boolean]
  | [command: 'terminal_focus', arguments_: { identifier: number }, expectedClose: () => boolean];

export async function updateTerminal(...[command, arguments_, expectedClose]: TerminalUpdate): Promise<void> {
  try { await invoke(command, arguments_); }
  catch (error) {
    if (String(error) === 'Terminal closed.' && expectedClose()) return;
    throw error;
  }
}
