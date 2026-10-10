# Clean

Clean temporary files. Currently unimplemented. See [guide](../guide.md).

```bash
$ condor clean
$ condor clean --all
```

| Name                      | Flag    | Type | Default |
| ------------------------- | ------- | ---- | ------- |
| [All](#all---all)         | `--all` |      |         |

## All `--all`

Clean all temporary files.

> [!NOTE]
> `clean` is a stub (`todo!()` in `src/lib.rs`). Manually remove the `--temp` directory for now.

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-clean-light.avif"><picture><source srcset="../media/tui/help-clean-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-clean-light.avif" alt="condor clean --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor clean --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-clean-verbose-light.avif"><picture><source srcset="../media/tui/help-clean-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-clean-verbose-light.avif" alt="condor clean --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor clean --help --verbose</p>
    </td>
  </tr>
</table>
