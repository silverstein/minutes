"use client";

import { useEffect } from "react";
import { trackAcquisition } from "@/lib/acquisition";

export function AcquisitionEvents() {
  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      if (!(event.target instanceof Element)) return;
      const link = event.target.closest<HTMLAnchorElement>("a[data-download-target]");
      const target = link?.dataset.downloadTarget;
      if (target === "mac" || target === "windows") {
        trackAcquisition("download_intent", target);
      }
    };
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);
  return null;
}
