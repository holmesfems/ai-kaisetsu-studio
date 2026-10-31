import React from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
// 非公開の見た目の上書き。公開の既定（styles.css）より後に読む（見つからなければ空。vite.config.ts）
import "@private-ui-css";
import App from "./App";

createRoot(document.getElementById("root")!).render(<App />);
