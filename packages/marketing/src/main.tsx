import { marketingTelemetry } from "./marketing-instrument";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { MarketingSite } from "./MarketingSite";
import "./marketing-site.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("Marketing site root element is missing");
}

createRoot(root, marketingTelemetry?.marketingReactErrors).render(
  <StrictMode>
    <MarketingSite />
  </StrictMode>,
);

marketingTelemetry?.recordMarketingBootstrap();
