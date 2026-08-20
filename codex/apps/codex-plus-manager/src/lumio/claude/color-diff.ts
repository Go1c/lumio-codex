export type DiffTag = "same" | "add" | "del";

export type TaggedLine = {
  tag: DiffTag;
  text: string;
};

function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function lcsTable(left: string[], right: string[]): number[][] {
  const table = Array.from({ length: left.length + 1 }, () =>
    Array<number>(right.length + 1).fill(0),
  );
  for (let i = 1; i <= left.length; i += 1) {
    for (let j = 1; j <= right.length; j += 1) {
      table[i][j] =
        left[i - 1] === right[j - 1]
          ? table[i - 1][j - 1] + 1
          : Math.max(table[i - 1][j], table[i][j - 1]);
    }
  }
  return table;
}

export function tagColorDiff(local: string, remote: string): TaggedLine[] {
  const left = splitLines(local);
  const right = splitLines(remote);
  const table = lcsTable(left, right);
  const reversed: TaggedLine[] = [];
  let i = left.length;
  let j = right.length;
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && left[i - 1] === right[j - 1]) {
      reversed.push({ tag: "same", text: left[i - 1] });
      i -= 1;
      j -= 1;
    } else if (j > 0 && (i === 0 || table[i][j - 1] >= table[i - 1][j])) {
      reversed.push({ tag: "add", text: right[j - 1] });
      j -= 1;
    } else {
      reversed.push({ tag: "del", text: left[i - 1] });
      i -= 1;
    }
  }
  return reversed.reverse();
}
