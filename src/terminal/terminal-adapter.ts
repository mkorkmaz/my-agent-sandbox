import { invoke } from '@tauri-apps/api/core';

export type TerminalRectangle = { x: number; y: number; width: number; height: number };
export type TerminalLayout = TerminalRectangle & { visible: boolean; overlay?: TerminalRectangle };
type TerminalUpdate =
  | [command: 'terminal_layout', arguments_: { identifier: number; layout: TerminalLayout }, expectedClose: () => boolean]
  | [command: 'terminal_focus', arguments_: { identifier: number }, expectedClose: () => boolean];

export async function updateTerminal(...[command, arguments_, expectedClose]: TerminalUpdate): Promise<void> {
  try { await invoke(command, arguments_); }
  catch (error) {
    if (String(error) === 'Terminal closed.' && expectedClose()) return;
    throw error;
  }
}
