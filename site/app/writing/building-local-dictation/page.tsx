import type { Metadata } from "next";
import { blogPostingSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "Building local dictation: the model was the easy part | minutes",
  description:
    "A build log of local dictation in Minutes: the Parakeet v3 swap, getting text into the right app, the bugs that hid behind other bugs, and two features we tested and did not ship.",
  alternates: {
    canonical: "/writing/building-local-dictation",
  },
};

const linkClass =
  "text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]";

const headingClass =
  "pt-4 font-serif text-[26px] leading-tight tracking-[-0.02em] text-[var(--text)]";

const engines = {
  headers: ["Engine", "Words differed", "Commands lost", "Median decode"],
  rows: [
    ["Whisper small", "6.9%", "8", "168 ms"],
    ["Parakeet v3", "2.1%", "0", "51 ms"],
  ],
};

function Table({ headers, rows }: { headers: string[]; rows: string[][] }) {
  return (
    <div className="overflow-x-auto rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
      <table className="min-w-full border-collapse text-left">
        <thead>
          <tr className="border-b border-[color:var(--border)]">
            {headers.map((h) => (
              <th
                key={h}
                className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]"
              >
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row[0]} className="border-b border-[color:var(--border)] last:border-b-0">
              {row.map((cell, i) => (
                <td
                  key={`${row[0]}-${headers[i]}`}
                  className={`px-4 py-3 font-mono text-[13px] ${
                    i === 0 ? "text-[var(--text)]" : "text-[var(--text-secondary)]"
                  }`}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function Post() {
  return (
    <div className="mx-auto max-w-[680px] px-6 pb-16 sm:px-8">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(blogPostingSchema({ metadata, headline: "The model was the easy part", path: "/writing/building-local-dictation", datePublished: "2026-11-03", dateModified: "2026-11-03" })) }}
      />
      <nav className="flex items-center justify-between border-b border-[color:var(--border)] py-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-x-6 text-sm text-[var(--text-secondary)]">
          <a href="/writing" className="hover:text-[var(--accent)]">
            Writing
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="hover:text-[var(--accent)]"
          >
            GitHub
          </a>
        </div>
      </nav>

      <article className="pt-14">
        <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
          2026-11-03 · Build log · Mat Silverstein
        </p>
        <h1 className="font-serif text-[36px] leading-[1.05] tracking-[-0.04em] text-[var(--text)] sm:text-[42px]">
          The model was the easy part
        </h1>
        <p className="mt-4 text-[18px] leading-[1.6] text-[var(--text-secondary)]">
          A build log of local dictation in Minutes.
        </p>

        <div className="mt-8 space-y-5 text-[16px] leading-[1.75] text-[var(--text-secondary)]">
          <p>
            Minutes has had dictation since March. You hold a key, talk, and the words land
            wherever your cursor is. All of it runs on your Mac. Going in, I assumed the speech
            model would be the hard part. It turned out to be the smallest part of the work. Most
            of the time went into what surrounds the model: getting text into the right app, being
            honest about whether it got there, and not shipping features that only worked in a
            demo.
          </p>

          <h2 className={headingClass}>The model</h2>
          <p>
            For most of this year, dictation used Whisper small. In October I moved the final pass
            to NVIDIA&apos;s Parakeet v3. I tested it on 39 of my own saved dictation clips. There
            is no hand-checked transcript for those, so Whisper medium served as the reference.
          </p>

          <Table headers={engines.headers} rows={engines.rows} />

          <p>
            Whisper small returned nothing at all for eight short commands like &quot;Done.&quot;
            and &quot;Proceed.&quot; On one 1.3-second clip it wrote 감사합니다, Korean for
            &quot;thank you.&quot; Parakeet did neither. In the app, the Parakeet pass took 127 to
            192&nbsp;ms on real dictations.
          </p>
          <p>
            Since late October, Parakeet v3 is the default wherever it is installed and your
            language is one it covers, and you can install it from inside the app. It has one clear
            weakness: rare names. It wrote &quot;Garek&quot; for &quot;Garrett,&quot; a name
            Whisper small got right. Adding the misspellings you see to your vocabulary fixes
            those. The full engine comparison is in{" "}
            <a href="/writing/whisper-cpp-vs-parakeet-cpp" className={linkClass}>
              Whisper vs Parakeet v3 for local transcription
            </a>
            .
          </p>
          <p>That swap took about a week. Everything below took months.</p>

          <h2 className={headingClass}>Text has to land where you are</h2>
          <p>
            Dictation is only useful if the words show up in the field you were typing in. On a Mac
            there are two ways to do that. Minutes can write into the focused control through
            Accessibility, or it can put the text on the clipboard and press Cmd-V for you.
          </p>
          <p>
            Writing through Accessibility is cleaner, and it sometimes lies. In Ghostty, macOS
            reported the write as a success while the prompt received nothing. The terminal built
            into Minutes did the same thing. So terminals always get the paste path now, and
            Minutes only says Typed when it can see that the text actually changed. Otherwise it
            says Pasted or Copied, and either way the words stay on your clipboard.
          </p>
          <p>
            &quot;Where you are&quot; also means the exact window you started in. If you have two
            windows of the same app open, the text goes to the one you were in.
          </p>
          <p>
            Other platforms get less. Linux on X11 gets a paste that Minutes cannot verify. Windows
            and Wayland get the clipboard, and Minutes tells you that instead of pretending.
          </p>

          <h2 className={headingClass}>Bugs that hid behind other bugs</h2>
          <p>
            With the default settings, every dictation pasted twice. It stayed hidden because the
            paste keystroke does nothing until you grant Accessibility permission. Granting the
            permission is what revealed it.
          </p>
          <p>
            The app also heard itself. The start sound played just after the microphone opened, so
            one transcript began with &quot;[MUSIC PLAYING].&quot; Dictation starts silently now.
          </p>
          <p>
            Some errors pointed in the wrong direction. A missing model file showed &quot;mic
            silent, check input device.&quot; The microphone, the permission, the signature and the
            device were all fine. Minutes now checks for the model before it opens the microphone,
            and offers a download button if it is missing. In another case, macOS never showed its
            microphone prompt, and dictation sat waiting for 59 seconds while Escape did nothing.
            Now Minutes asks for access itself and gives up after 30 seconds.
          </p>
          <p>
            None of these show up in a benchmark. You find them by dictating all day and reading
            the logs when something feels off.
          </p>

          <h2 className={headingClass}>What I didn&apos;t ship</h2>
          <p>Two features looked finished in a demo and failed their tests.</p>
          <p>
            The first was showing words on screen while you speak. The bar was a first useful word
            in under 700&nbsp;ms for 95% of runs, at most 30% of words wrong, and never rewriting
            text already on screen. I tried five setups across four engines. Apple&apos;s
            DictationTranscriber came closest, at 751&nbsp;ms. Whisper tiny was quick and invented
            things, including &quot;Maddened Wesley of the World.&quot; Nothing passed, so the
            dictation pill shows a waveform. A fast wrong guess on screen is more distracting than
            no text at all.
          </p>
          <p>
            The second was self-correction: when you say &quot;actually&quot; and fix yourself
            mid-sentence, drop the part you corrected. I tested a rule that treats
            &quot;actually&quot; after a dash as a correction. It caught all 12 real corrections in
            the test set. It also rewrote 9 of the 20 sentences that were not corrections. The bar
            was zero wrong rewrites, because a dictation tool that quietly changes what you said is
            worse than one that leaves a word in. So &quot;actually&quot; stays literal, and you
            say &quot;scratch that&quot; when you want to take something back.
          </p>
          <p>
            Cleanup does remove filler it can be sure about. From the tests: &quot;Alright, this is
            a dictation app test. Well, I mean, yeah, let&apos;s try that again.&quot; becomes
            &quot;Alright, this is a dictation app test. Let&apos;s try that again.&quot;
          </p>

          <h2 className={headingClass}>Where it stands</h2>
          <p>
            On an Apple Silicon Mac with Parakeet v3 installed, Parakeet writes the final text.
            Whisper covers everything else and steps in if Parakeet fails. Every dictation is saved
            as a Markdown file you own. The next release adds a hook that can hand each dictation to
            your own agent, a script or a webhook.
          </p>
          <p>
            What is still open: rare names on Parakeet, typing at the cursor on Windows, and a
            side-by-side test against Wispr Flow on real speech. I haven&apos;t run that
            comparison, so I won&apos;t claim we match it.
          </p>
          <p>
            The accuracy of a model is a number you can look up. Whether the text lands, and
            whether the app tells you the truth about it, you only learn by using it every day. The
            code, the evaluations and the failed experiments are all{" "}
            <a href="https://github.com/silverstein/minutes" className={linkClass}>
              on GitHub
            </a>
            .
          </p>
        </div>
      </article>
    </div>
  );
}
