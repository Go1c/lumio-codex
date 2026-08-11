import { createRoot } from "react-dom/client";
import { LumioApp } from "./LumioApp";
import "./lumio-shell.css";
import "@fontsource/jetbrains-mono";

const app = document.getElementById("app");
if (app instanceof HTMLElement) createRoot(app).render(<LumioApp />);
