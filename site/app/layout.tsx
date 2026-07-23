import type { Metadata } from "next";
import { Instrument_Serif } from "next/font/google";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "./globals.css";

const instrumentSerif = Instrument_Serif({
  subsets: ["latin"],
  weight: "400",
  style: ["normal", "italic"],
  variable: "--font-instrument-serif",
  display: "swap",
});

export const metadata: Metadata = {
  title: "minutes — open-source conversation memory",
  description:
    "Record meetings, capture voice memos, search everything. Local transcription with whisper.cpp, structured markdown, Claude-native. Free forever.",
  metadataBase: new URL("https://useminutes.app"),
  alternates: { canonical: "/" },
  icons: {
    icon: [
      { url: "/favicon.svg", type: "image/svg+xml" },
    ],
  },
  openGraph: {
    title: "minutes — open-source conversation memory",
    description:
      "Record meetings, capture voice memos, ask your AI what was decided. Local transcription, structured markdown, free forever.",
    type: "website",
    url: "https://useminutes.app",
    siteName: "minutes",
  },
  twitter: {
    card: "summary",
    title: "minutes — open-source conversation memory",
    description:
      "Record meetings, capture voice memos, ask your AI what was decided. Local, free, MIT licensed.",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html
      lang="en"
      className={`${GeistSans.variable} ${GeistMono.variable} ${instrumentSerif.variable}`}
    >
      <head>
        <link rel="alternate" type="text/plain" href="/llms.txt" />
        <meta
          name="theme-color"
          media="(prefers-color-scheme: light)"
          content="#F8F4ED"
        />
        <meta
          name="theme-color"
          media="(prefers-color-scheme: dark)"
          content="#0D0D0B"
        />
      </head>
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
