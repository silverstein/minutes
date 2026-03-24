import type { Metadata } from "next";
import { GeistSans } from "geist/font/sans";
import { GeistMono } from "geist/font/mono";
import "./globals.css";

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
  other: {
    "theme-color": "#000000",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`}>
      <head>
        <link rel="alternate" type="text/plain" href="/llms.txt" />
      </head>
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
