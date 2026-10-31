import React from "react";
import { useS } from "../state";

export default function Toasts() {
  const toasts = useS((s) => s.toasts);
  return (
    <div className="toasts">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind ?? ""}`} title={t.detail}>{t.text}</div>
      ))}
    </div>
  );
}
