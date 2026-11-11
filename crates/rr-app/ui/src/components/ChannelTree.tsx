import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useApp } from "../store";
import { buildTree, type TreeNode } from "../util";
import { useHold } from "./useHold";

function HoldButton({ label, title, onDown, onUp }: { label: string; title: string; onDown: () => void; onUp: () => void }) {
  const hold = useHold(onDown, onUp);
  return (
    <button type="button" className="hold" title={title} aria-label={title} {...hold}>
      {label}
    </button>
  );
}

function Node({ node, depth }: { node: TreeNode; depth: number }) {
  const { t } = useTranslation();
  const status = useApp((s) => s.status);
  const here = status.own_channel === node.channel.id;
  const talking = new Set(status.speakers.filter((s) => s.active).map((s) => s.session));
  const remote = new Set(status.remote_ptt);

  return (
    <li>
      <div className={`channel${here ? " here" : ""}`} style={{ paddingInlineStart: `${depth * 14 + 8}px` }}>
        <button type="button" className="channel-name" onClick={() => void api.joinChannel(node.channel.id)} title={t("channels.join")}>
          {node.channel.name}
        </button>
        {!here ? (
          <span className="channel-actions">
            <HoldButton
              label="W"
              title={t("channels.whisper")}
              onDown={() => void api.whisper(node.channel.id, true)}
              onUp={() => void api.whisper(node.channel.id, false)}
            />
            <HoldButton
              label="L"
              title={t("channels.listen")}
              onDown={() => void api.listenIn(node.channel.id, true)}
              onUp={() => void api.listenIn(node.channel.id, false)}
            />
          </span>
        ) : null}
      </div>
      {node.users.length > 0 ? (
        <ul className="users">
          {node.users.map((u) => (
            <li
              key={u.session}
              className={`user${talking.has(u.session) ? " talking" : ""}`}
              style={{ paddingInlineStart: `${depth * 14 + 26}px` }}
            >
              <span className="dot" aria-hidden />
              <span className="user-name">{u.name}</span>
              {u.session === status.own_session ? <em>{t("channels.you")}</em> : null}
              {remote.has(u.session) ? <span className="badge on">{t("channels.remotePtt")}</span> : null}
              {u.self_mute ? <span className="badge">🔇</span> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {node.children.length > 0 ? (
        <ul>
          {node.children.map((c) => (
            <Node key={c.channel.id} node={c} depth={depth + 1} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export function ChannelTree() {
  const { t } = useTranslation();
  const status = useApp((s) => s.status);
  const tree = useMemo(() => buildTree(status.channels, status.users), [status.channels, status.users]);

  return (
    <section className="channels" aria-label={t("channels.title")}>
      <h2>{t("channels.title")}</h2>
      {tree.length === 0 ? (
        <p className="muted">{t("channels.empty")}</p>
      ) : (
        <ul className="tree">
          {tree.map((n) => (
            <Node key={n.channel.id} node={n} depth={0} />
          ))}
        </ul>
      )}
    </section>
  );
}
