import { StrictMode } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { MarketingSite } from "./MarketingSite";
import "./marketing-site.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("Marketing site root element is missing");
}

// The prerendered markup is for crawlers and first paint. The client render replaces it
// instead of hydrating, because the prerender cannot know the visitor's theme.
const reactRoot = createRoot(root);
flushSync(() => {
  reactRoot.render(
    <StrictMode>
      <MarketingSite />
    </StrictMode>,
  );
});
delete root.dataset.prerendered;
