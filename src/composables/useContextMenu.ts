import { Menu } from '@tauri-apps/api/menu';

/** A line of a context menu: something to do, a divider, or a menu of more. */
export type MenuLine =
  | { text: string; action: () => void; enabled?: boolean }
  | { text: string; items: MenuLine[]; enabled?: boolean }
  | 'separator';

type Built = NonNullable<NonNullable<Parameters<typeof Menu.new>[0]>['items']>[number];

function build(line: MenuLine): Built {
  if (line === 'separator') return { item: 'Separator' };
  if ('items' in line) return { text: line.text, enabled: line.enabled, items: line.items.map(build) };
  return { text: line.text, enabled: line.enabled, action: line.action };
}

/** Shows the system's own context menu under the pointer. */
export async function showContextMenu(lines: MenuLine[]) {
  const menu = await Menu.new({ items: lines.map(build) });
  await menu.popup();
}
