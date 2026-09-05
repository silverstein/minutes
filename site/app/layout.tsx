import type { Metadata } from "next";
import { WebsiteAnalytics } from "@/components/website-analytics";
import { AcquisitionEvents } from "@/components/acquisition-events";
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

// Title and description target the queries that actually drive impressions
// here ("minutes app", "minute app"), which were converting at 1.5% CTR from
// positions 4 to 7. Free and open source lead because they are the two claims
// no paid competitor in that SERP can make. Every negative below is literal:
// there is no signup, no API key is required, and the app is MIT licensed.
// Deliberately not "no cloud": optional summarization can use a cloud LLM.
//
// The category noun is "conversation memory", not "meeting notes". This title
// is the strongest entity signal on the domain and is what an LLM reads to
// answer "what is Minutes", so it has to match the SoftwareApplication schema
// in lib/schema.ts rather than file the product into the one category where
// the comparison pages openly concede competitors are better. Google may
// rewrite the SERP title when it does not match a generic "minutes app"
// query; that costs the display but not the entity signal, which is the
// asset worth protecting. Dictation is named because it is half the wedge
// and "meeting notes" excludes it.
export const metadata: Metadata = {
  title: "Minutes: free, open-source conversation memory app",
  description:
    "Meetings, calls, voice memos, and dictation, transcribed on your own machine and saved as markdown you own. No account, no API keys, no subscription.",
  metadataBase: new URL("https://useminutes.app"),
  alternates: { canonical: "/" },
  icons: {
    icon: [
      { url: "/favicon.svg", type: "image/svg+xml" },
    ],
  },
  openGraph: {
    title: "Minutes: free, open-source conversation memory app",
    description:
      "Meetings, calls, voice memos, and dictation, transcribed on your own machine and saved as markdown you own. Free and open source.",
    type: "website",
    url: "https://useminutes.app",
    siteName: "minutes",
  },
  twitter: {
    card: "summary",
    title: "Minutes: free, open-source conversation memory app",
    description:
      "Meetings, calls, voice memos, and dictation, transcribed on your own machine. Markdown you own. Free, MIT licensed.",
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
      <body className="font-sans antialiased">
        {children}
        <AcquisitionEvents />
        <WebsiteAnalytics />
      </body>
    </html>
  );
}
