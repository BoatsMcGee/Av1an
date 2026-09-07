# sequence_config.noise_detector

`sequence_config.noise_detector` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`detect-noise`](../../../commands/detect-noise.md) (`null` on `init`). CLI syntax in [Filters](../../../types/filters.md), JSON shapes in [Filters](../../../types/filters.md#json-shape).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |
| `reference_filters` | Array of `VapourSynthFilter` objects | Yes | See below | Reference denoise filter chain. CLI `--reference-filters` |
| `denoised_filters` | Array of `VapourSynthFilter` objects | Yes | See below | Strong denoise filter chain. CLI `--denoised-filters` |

CLI mapping: `detect-noise --reference-filters <filters> --denoised-filters <filters>`. Each CLI string is parsed into one object in the array.

Default `reference_filters` (`wnnm:sigma=3.0,0.0,0.0;`):

```json
[
    {
        "WNNM": {
            "sigma": [3.0, 0.0, 0.0],
            "block_size": null,
            "block_step": null,
            "group_size": null,
            "bm_range": null,
            "radius": null,
            "ps_num": null,
            "ps_range": null,
            "residual": null,
            "adaptive_aggregation": null
        }
    }
]
```

Default `denoised_filters` (`wnnm:sigma=6.0,0.0,0.0;`):

```json
[
    {
        "WNNM": {
            "sigma": [6.0, 0.0, 0.0],
            "block_size": null,
            "block_step": null,
            "group_size": null,
            "bm_range": null,
            "radius": null,
            "ps_num": null,
            "ps_range": null,
            "residual": null,
            "adaptive_aggregation": null
        }
    }
]

```

Example:

```json
{
    "input": null,
    "reference_filters": [
        {
            "WNNM": {
                "sigma": [3.0, 0.0, 0.0],
                "block_size": null,
                "block_step": null,
                "group_size": null,
                "bm_range": null,
                "radius": null,
                "ps_num": null,
                "ps_range": null,
                "residual": null,
                "adaptive_aggregation": null
            }
        }
    ],
    "denoised_filters": [
        {
            "WNNM": {
                "sigma": [6.0, 0.0, 0.0],
                "block_size": null,
                "block_step": null,
                "group_size": null,
                "bm_range": null,
                "radius": null,
                "ps_num": null,
                "ps_range": null,
                "residual": null,
                "adaptive_aggregation": null
            }
        }
    ]
}
```
