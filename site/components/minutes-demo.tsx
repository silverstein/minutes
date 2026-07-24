"use client";

import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  spring,
  useVideoConfig,
  Sequence,
} from "remotion";
import { MINUTES_MCP_TOOL_COUNT } from "@/lib/release";

const BG = "#F8F4ED";
const FG = "#1A1916";
const DIM = "#8C8880";
const DIMMER = "#BDB9B0";
const GREEN = "#2E7D46";
const ACCENT = "#C96B4E";
const PURPLE = "#BF5AF2";
const ORANGE = "#D4832A";
const RED = "#C0392B";
const BORDER = "rgba(0,0,0,0.09)";
const TERMINAL_BG = "#EFEBE2";
const BAR_BG = "rgba(0,0,0,0.03)";

const FONT_MAIN = 15;
const FONT_SMALL = 12;
const FONT_TITLE = 12;

const TerminalWindow: React.FC<{
  children: React.ReactNode;
  title?: string;
}> = ({ children, title = "Terminal" }) => (
  <div
    style={{
      background: TERMINAL_BG,
      borderRadius: 12,
      border: `1px solid ${BORDER}`,
      overflow: "hidden",
      width: "100%",
      height: "100%",
      display: "flex",
      flexDirection: "column",
    }}
  >
    <div
      style={{
        height: 40,
        background: BAR_BG,
        borderBottom: `1px solid ${BORDER}`,
        display: "flex",
        alignItems: "center",
        paddingLeft: 16,
        gap: 8,
      }}
    >
      <div style={{ width: 12, height: 12, borderRadius: "50%", background: RED }} />
      <div style={{ width: 12, height: 12, borderRadius: "50%", background: ORANGE }} />
      <div style={{ width: 12, height: 12, borderRadius: "50%", background: GREEN }} />
      <span
        style={{
          marginLeft: 14,
          color: DIMMER,
          fontSize: FONT_TITLE,
          fontFamily: "var(--font-mono), SF Mono, Menlo, monospace",
        }}
      >
        {title}
      </span>
    </div>
    <div
      style={{
        flex: 1,
        padding: "20px 24px",
        fontFamily: "var(--font-mono), SF Mono, Menlo, Consolas, monospace",
        fontSize: FONT_MAIN,
        lineHeight: 1.7,
        color: FG,
        overflow: "hidden",
        textAlign: "left" as const,
      }}
    >
      {children}
    </div>
  </div>
);

const TypedText: React.FC<{
  text: string;
  startFrame: number;
  speed?: number;
  color?: string;
  bold?: boolean;
}> = ({ text, startFrame, speed = 2, color = FG, bold = false }) => {
  const frame = useCurrentFrame();
  const elapsed = frame - startFrame;
  if (elapsed < 0) return null;
  const chars = Math.min(Math.floor(elapsed / speed), text.length);
  return (
    <span style={{ color, fontWeight: bold ? 600 : 400 }}>
      {text.slice(0, chars)}
    </span>
  );
};

const FadeIn: React.FC<{
  children: React.ReactNode;
  delay?: number;
}> = ({ children, delay = 0 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const opacity = spring({ frame: frame - delay, fps, config: { damping: 20 } });
  return <div style={{ opacity }}>{children}</div>;
};

const Cursor: React.FC<{ visible?: boolean }> = ({ visible = true }) => {
  const frame = useCurrentFrame();
  const blink = Math.floor(frame / 8) % 2 === 0;
  if (!visible || !blink) return null;
  return (
    <span
      style={{
        display: "inline-block",
        width: 9,
        height: 18,
        background: GREEN,
        marginLeft: 2,
        verticalAlign: "middle",
      }}
    />
  );
};

const Line: React.FC<{ mt?: number; children: React.ReactNode }> = ({
  mt = 0,
  children,
}) => <div style={{ marginTop: mt }}>{children}</div>;

// Scene 1: Record + Transcribe
const Scene1: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <TerminalWindow title="minutes — record a meeting">
      <Line>
        <span style={{ color: GREEN }}>$ </span>
        <TypedText text="minutes record" startFrame={8} color={FG} bold />
        <Cursor visible={frame < 25} />
      </Line>
      {frame > 28 && (
        <FadeIn delay={28}>
          <Line mt={12}>
            <span style={{ color: RED }}>● </span>
            <span style={{ color: DIM }}>Recording... streaming transcription active </span>
            <span style={{ color: FG }}>
              {Math.min(Math.floor((frame - 28) / 3), 42)}s
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 62 && (
        <Line mt={20}>
          <span style={{ color: GREEN }}>$ </span>
          <TypedText text="minutes stop" startFrame={64} color={FG} bold />
        </Line>
      )}
      {frame > 80 && (
        <FadeIn delay={80}>
          <Line mt={12}>
            <span style={{ color: DIM }}>Transcribing (GPU accelerated)... extracting decisions...</span>
          </Line>
        </FadeIn>
      )}
      {frame > 96 && (
        <FadeIn delay={96}>
          <Line mt={8}>
            <span style={{ color: GREEN, fontWeight: 600 }}>
              ✓ ~/meetings/2026-03-24-q2-planning-call.md
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 108 && (
        <FadeIn delay={108}>
          <Line mt={6}>
            <span style={{ color: DIM, fontSize: FONT_SMALL }}>
              42s · 2 speakers · 3 action items · 2 decisions
            </span>
          </Line>
        </FadeIn>
      )}
    </TerminalWindow>
  );
};

// Scene 2: Dictation
const Scene2: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <TerminalWindow title="minutes — dictation mode">
      <Line>
        <span style={{ color: DIM, fontSize: FONT_SMALL }}>
          Hold hotkey to speak · text goes to clipboard + daily note
        </span>
      </Line>
      {frame > 12 && (
        <FadeIn delay={12}>
          <Line mt={20}>
            <span style={{ color: RED }}>● </span>
            <span style={{ color: DIM }}>Listening...</span>
          </Line>
        </FadeIn>
      )}
      {frame > 30 && (
        <FadeIn delay={30}>
          <Line mt={16}>
            <span style={{ color: GREEN }}>✓ </span>
            <span style={{ color: FG }}>
              &ldquo;We should switch consultants to monthly billing
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 40 && (
        <FadeIn delay={40}>
          <Line mt={2}>
            <span style={{ color: FG }}>
              {"  "}instead of annual — their revenue is project-based&rdquo;
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 55 && (
        <FadeIn delay={55}>
          <Line mt={16}>
            <span style={{ color: ACCENT }}>Copied to clipboard</span>
          </Line>
        </FadeIn>
      )}
      {frame > 65 && (
        <FadeIn delay={65}>
          <Line mt={4}>
            <span style={{ color: DIM }}>Appended to daily note</span>
          </Line>
        </FadeIn>
      )}
      {frame > 80 && (
        <FadeIn delay={80}>
          <Line mt={16}>
            <span style={{ color: DIMMER, fontSize: FONT_SMALL }}>
              Local whisper · no cloud · no API key · works offline
            </span>
          </Line>
        </FadeIn>
      )}
    </TerminalWindow>
  );
};

// Scene 3: Phone → Desktop
const Scene3: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <TerminalWindow title="minutes watch — phone → desktop">
      <Line>
        <span style={{ color: DIM }}>Watching for voice memos...</span>
      </Line>
      {frame > 15 && (
        <FadeIn delay={15}>
          <Line mt={16}>
            <span style={{ color: FG, fontWeight: 600 }}>pricing-idea.m4a </span>
            <span style={{ color: DIM }}>synced from iPhone</span>
          </Line>
        </FadeIn>
      )}
      {frame > 35 && (
        <FadeIn delay={35}>
          <Line mt={12}>
            <span style={{ color: DIM }}>{"   "}duration: </span>
            <span style={{ color: ORANGE, fontWeight: 600 }}>46s </span>
            <span style={{ color: DIM }}>→ </span>
            <span style={{ color: GREEN }}>fast memo pipeline</span>
          </Line>
        </FadeIn>
      )}
      {frame > 50 && (
        <FadeIn delay={50}>
          <Line mt={4}>
            <span style={{ color: DIM }}>{"   "}transcribing... </span>
            {frame > 65 && (
              <span style={{ color: GREEN, fontWeight: 600 }}>✓ 4.2s</span>
            )}
          </Line>
        </FadeIn>
      )}
      {frame > 72 && (
        <FadeIn delay={72}>
          <Line mt={12}>
            <span style={{ color: GREEN, fontWeight: 600 }}>
              ✓ ~/meetings/memos/pricing-idea.md
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 90 && (
        <FadeIn delay={90}>
          <Line mt={16}>
            <div
              style={{
                padding: "10px 14px",
                background: "rgba(255,255,255,0.03)",
                borderRadius: 8,
                border: `1px solid ${BORDER}`,
                display: "inline-block",
              }}
            >
              <span style={{ color: PURPLE, fontWeight: 600 }}>Ghost context: </span>
              <span style={{ color: FG }}>
                Claude sees this in your next session
              </span>
            </div>
          </Line>
        </FadeIn>
      )}
    </TerminalWindow>
  );
};

// Scene 4: AI Recall
const Scene4: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <TerminalWindow title="Claude — what was that idea?">
      <Line>
        <span style={{ color: ACCENT, fontWeight: 600 }}>you: </span>
        <TypedText
          text="what was that pricing idea I had while walking?"
          startFrame={8}
          speed={1}
          color={FG}
        />
        <Cursor visible={frame > 8 && frame < 60} />
      </Line>
      {frame > 65 && (
        <FadeIn delay={65}>
          <Line mt={20}>
            <span style={{ color: PURPLE, fontWeight: 600 }}>claude: </span>
            <span style={{ color: FG }}>
              Found your voice memo from today (46s, iPhone):
            </span>
          </Line>
        </FadeIn>
      )}
      {frame > 78 && (
        <FadeIn delay={78}>
          <div
            style={{
              marginTop: 12,
              marginLeft: 16,
              padding: "12px 16px",
              background: "rgba(255,255,255,0.03)",
              borderRadius: 8,
              borderLeft: `3px solid ${PURPLE}`,
              color: FG,
              lineHeight: 1.6,
            }}
          >
            <div>Switch consultants to monthly billing.</div>
            {frame > 88 && <div>Revenue is project-based, not recurring.</div>}
            {frame > 98 && <div>Test with next 3 signups, compare retention.</div>}
          </div>
        </FadeIn>
      )}
      {frame > 112 && (
        <FadeIn delay={112}>
          <Line mt={14}>
            <span style={{ color: ORANGE, fontWeight: 600 }}>Action: </span>
            <span style={{ color: FG }}>
              Test monthly billing with next 3 consultant signups
            </span>
          </Line>
        </FadeIn>
      )}
    </TerminalWindow>
  );
};

// Scene 5: Stats banner
const Scene5: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const stats = [
    { label: "interface", value: "CLI", color: GREEN },
    { label: "transcription", value: "local", color: ACCENT },
    { label: "MCP tools", value: String(MINUTES_MCP_TOOL_COUNT), color: PURPLE },
    { label: "license", value: "MIT", color: ORANGE },
  ];

  return (
    <AbsoluteFill
      style={{
        background: BG,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        padding: 48,
      }}
    >
      <div
        style={{
          opacity: spring({ frame, fps, config: { damping: 15 } }),
          fontSize: 52,
          fontWeight: 700,
          color: FG,
          fontFamily: "var(--font-sans), -apple-system, sans-serif",
          letterSpacing: -2,
        }}
      >
        minutes
      </div>
      <div
        style={{
          opacity: spring({ frame: frame - 6, fps, config: { damping: 15 } }),
          fontSize: 18,
          color: DIM,
          fontFamily: "var(--font-sans), -apple-system, sans-serif",
          marginTop: 10,
        }}
      >
        your AI remembers every conversation
      </div>

      <div style={{ display: "flex", gap: 40, marginTop: 44 }}>
        {stats.map((s, i) => (
          <div
            key={s.label}
            style={{
              opacity: spring({
                frame: frame - 16 - i * 5,
                fps,
                config: { damping: 15 },
              }),
              textAlign: "center",
            }}
          >
            <div
              style={{
                fontSize: 30,
                fontWeight: 700,
                color: s.color,
                fontFamily: "var(--font-mono), SF Mono, monospace",
              }}
            >
              {s.value}
            </div>
            <div
              style={{
                fontSize: 13,
                color: DIM,
                marginTop: 6,
                fontFamily: "var(--font-sans), -apple-system, sans-serif",
              }}
            >
              {s.label}
            </div>
          </div>
        ))}
      </div>

      {frame > 45 && (
        <div
          style={{
            opacity: spring({ frame: frame - 45, fps, config: { damping: 15 } }),
            marginTop: 40,
            fontSize: 16,
            color: ACCENT,
            fontFamily: "var(--font-mono), SF Mono, monospace",
          }}
        >
          github.com/silverstein/minutes
        </div>
      )}
    </AbsoluteFill>
  );
};

// Main composition — 5 scenes, 630 frames @ 15fps = 42s
export const MinutesDemo: React.FC = () => {
  return (
    <AbsoluteFill style={{ background: BG, padding: 16, textAlign: "left" }}>
      <Sequence from={0} durationInFrames={135}>
        <Scene1 />
      </Sequence>
      <Sequence from={135} durationInFrames={105}>
        <Scene2 />
      </Sequence>
      <Sequence from={240} durationInFrames={135}>
        <Scene3 />
      </Sequence>
      <Sequence from={375} durationInFrames={150}>
        <Scene4 />
      </Sequence>
      <Sequence from={525} durationInFrames={105}>
        <Scene5 />
      </Sequence>
    </AbsoluteFill>
  );
};
