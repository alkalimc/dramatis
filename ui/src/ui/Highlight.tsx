import { pieces } from "../lib/snippet";

/** `text` with the hits of the search terms marked. */
export function Highlight({ text, terms }: { text: string; terms: string[] }) {
  return (
    <>
      {pieces(text, terms).map((p, i) => (p.hit ? <mark key={i}>{p.text}</mark> : p.text))}
    </>
  );
}
