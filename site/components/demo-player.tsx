"use client";

import { Player } from "@remotion/player";
import { MinutesDemo } from "./minutes-demo";

export function DemoPlayer() {
  return (
    <div className="mx-auto w-full max-w-[720px] overflow-hidden rounded-[8px] border border-[color:var(--border)] text-left shadow-[var(--shadow-panel)]" style={{ maxHeight: "min(55vw, 380px)" }}>
      <Player
        component={MinutesDemo}
        durationInFrames={630}
        fps={15}
        compositionWidth={900}
        compositionHeight={550}
        style={{ width: "100%" }}
        autoPlay
        loop
        acknowledgeRemotionLicense
      />
    </div>
  );
}
