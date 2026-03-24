"use client";

import { Player } from "@remotion/player";
import { MinutesDemo } from "./minutes-demo";

export function DemoPlayer() {
  return (
    <div className="w-full max-w-[720px] mx-auto rounded-xl overflow-hidden border border-white/[0.06] text-left" style={{ maxHeight: "min(55vw, 380px)" }}>
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
