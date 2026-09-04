import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../app/globals.css";
import Page from "../app/page";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Page />
  </StrictMode>,
);
