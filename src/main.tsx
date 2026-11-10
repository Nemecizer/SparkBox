import React from "react";
import ReactDOM from "react-dom/client";
// Global styles first, so component stylesheets (window chrome, shell) can override them.
import "uplot/dist/uPlot.min.css";
import "./styles.css";
import App from "./App";
import { startCore } from "./components/CoreBanner";
import { parsePopout } from "./lib/popout";
import { PopoutRoot } from "./wm/PopoutRoot";

// The data stores: a failure shows in the core banner (with Retry) instead of vanishing.
startCore();

// index.html?popout=<kind> is one window popped out of the desk; anything else is the app.
const popout = parsePopout(window.location.search);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {popout ? <PopoutRoot route={popout} /> : <App />}
  </React.StrictMode>,
);
