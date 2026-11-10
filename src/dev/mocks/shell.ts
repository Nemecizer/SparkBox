// The desktop shell's commands for the browser preview: the native menu bar
// (there is none in a browser, so these just remember what they were told) and
// the per-window error report.
import type { MockHandlers } from "../../test/tauri";

/** What the page last told the menu, for poking at in the console. */
export const menuMock = {
  windows: [] as unknown[],
  state: null as unknown,
  errors: [] as unknown[],
};

export const handlers: MockHandlers = {
  menu_set_windows: ({ windows }) => {
    menuMock.windows = windows;
    return null;
  },
  menu_set_state: ({ state }) => {
    menuMock.state = state;
    return null;
  },
  menu_close_main: () => null,
  menu_minimize_main: () => null,
  log_ui_error: (args) => {
    menuMock.errors.push(args);
    return null;
  },
};
