import React from "react";
import ReactDOM from "react-dom/client";
// Self-hosted fonts (bundled by Vite — no runtime CDN, works offline).
// Explicit .css paths so they resolve against vite/client's `*.css`
// module type (the bare specifier has no type declarations).
import "@fontsource-variable/inter/index.css";
import "@fontsource-variable/jetbrains-mono/index.css";
import App from "./App";
import ErrorBoundary from "./ErrorBoundary";
import "./index.css";
import "./hud.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary label="Vigla">
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
