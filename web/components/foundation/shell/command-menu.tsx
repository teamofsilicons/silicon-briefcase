"use client";

/**
 * The ⌘K command palette in Silicon UI's headerless Dialog, as on Silicon Accounts. Commands come from the registry
 * (lib/commands.ts): the shell's navigation and account commands plus whatever the current page adds.
 */
import { CommandPalette, type CommandItem } from "@teamofsilicons/silicon-ui/command-palette";
import { Dialog, DialogContent } from "@teamofsilicons/silicon-ui/dialog";
import { closeCommandPalette, setCommandPaletteOpen, useCommandPaletteOpen, useCommands } from "@/lib/commands";

export function CommandMenu() {
  const open = useCommandPaletteOpen();
  const commands = useCommands();
  const items: CommandItem[] = commands.map(({ id, label, description, group, keywords, icon, shortcut }) => ({ id, label, description, group, keywords, icon, shortcut }));
  const run = (item: CommandItem) => {
    closeCommandPalette();
    const command = commands.find(entry => entry.id === item.id);
    // Run after the dialog has started closing, so focus returns before the command moves it (navigation, a drawer).
    if (command) requestAnimationFrame(() => command.run());
  };
  return (
    <Dialog open={open} onOpenChange={setCommandPaletteOpen}>
      <DialogContent title="Search and jump" hideHeader placement="top">
        {open ? <CommandPalette items={items} placeholder="Search pages and actions" label="Search pages and actions" autoFocus onClose={closeCommandPalette} onSelect={run} /> : null}
      </DialogContent>
    </Dialog>
  );
}
