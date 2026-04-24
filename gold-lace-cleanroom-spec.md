# Gold Lace 2.2 clean-room reconstruction spec

Date: 2026-04-24

Source basis: static analysis of `unpacked-4.exe`, extracted help/resources, and `palettes.plt`.
Original packed `.scr` / `.exe` files were not executed.

This is a consolidated implementation-oriented spec distilled from `working-notes.md`.  It is not byte-for-byte source recovery; it records the recovered algorithms, data layouts, and formula families needed for a faithful clean-room clone.

## 1. User-visible model

Gold Lace generates procedural abstract images with four style families:

- **Embossing**: default relief-like style; no explicit style byte set.
- **Ribbons**: banded/twisted strip style.
- **Rhombuses**: pattern structure gated by an extra rhombus-like comparison.
- **Mesh**: pattern structure gated by an extra mesh/stripe-difference comparison.

The documented settings map well to the recovered internals:

- **Complexity** controls harmonic/random frequency structure and many coefficient banks.
- **Relief** controls profile contrast/depth, especially for Embossing.
- **Movement** selects center-out / outside-in direction behavior.
- **Symmetry** chooses centered vs random/deformed placement.
- **Palette speed / contrast** influence palette motion and final color/gain handling.

## 2. Core constants and math helpers

### 2.1 Runtime constants

| Address | Value | Meaning |
|---:|---:|---|
| `0x4D14A0` | `32768` | legacy/base trig period; also used in coefficient formulas |
| `0x4D14A4` | `2*pi` | initialized as `32768 * (2*pi/32768)` |
| `0x4D14A8` | `1/sqrt(2)` | radial normalization scale |
| `0x4D14AC` | `3998.0` | profile-coordinate scale, effectively `4000 - 2` |
| `0x4D14B0` | `1/pi` | normalizes `abs(atan2(...))` |

Raster block constants around `0x40BD00`:

| Address | Value |
|---:|---:|
| `0x40BD00` | `1.0f` |
| `0x40BD04` | `0.5f` |
| `0x40BD08` | `pi` |
| `0x40BD18` | `0.0001` |
| `0x40BD2C` | `2.0` |
| `0x40BD34` | `0.1` |
| `0x40BD3C` | `65536.0f` |
| `0x40BD50` | `0.2` |
| `0x40BD58` | `0.05` |
| `0x40BD60` | `100.0f` |
| `0x40BD64` | `50.0f` |
| `0x40BD68` | `0.25f` |
| `0x40BD6C` | `30.0f` |
| `0x40BD70` | `10.0f` |

### 2.2 Helper identities

| Address | Helper |
|---:|---|
| `0x4C656C` | `fabs` |
| `0x4C657C` | floor-style integer helper |
| `0x4C65B0` | round-to-int helper |
| `0x4C6A68` | `pow` |
| `0x4C687C` | `log` |
| `0x4C7D7C` | `sin` |
| `0x4C7DC0` | `sqrt` |
| `0x4C7DFC` | `tanh` |
| `0x4C625C` | `cos` |
| `0x4C60FC` | `atan2` |
| `0x4C6640` | `hypot` |

### 2.3 Trig LUTs

Two 65536-entry float lookup tables are built at startup:

- `sin_lut[i]` at `0x4FD538`
- `cos_lut[i]` at `0x53D538`
- `angle = 2*pi*i/65536`
- indexes are usually `round(phase) & 0xffff`

## 3. Important global/UI mapping

| Global | Meaning |
|---:|---|
| `0x4D1460` | max complexity |
| `0x4D1464` | min complexity |
| `0x4D1468` | max relief |
| `0x4D146C` | min relief |
| `0x4D1470` | movement direction flag A |
| `0x4D1471` | movement direction flag B |
| `0x4D1472` | symmetry flag A |
| `0x4D1473` | symmetry flag B |
| `0x4D1474` | Ribbons style enabled |
| `0x4D1475` | Embossing/default style enabled |
| `0x4D1476` | Rhombuses style enabled |
| `0x4D1477` | Mesh style enabled |
| `0x4D1478..7C` | spatial/asymmetry double; fallback `0.96` if zero |
| `0x4D5238` | UI/config double used by vertical cutoff helper `0x408C58` |
| `0x4D5240` | UI/config double used by nonlinear RGB gain formula |

Nonlinear RGB/intensity gain:

```text
s = max(spatial_double, 1e-5)
base = 0.5 - 0.5 * tanh(log(s) * 122.549 / log(0.96) - 3124.9995)
G = *(double*)0x4D5240
m = round((1.038 - pow(abs(sin(0.37962*G + 11.834))
                      + abs(sin(1.631102533771764*G + 5.346)), 0.1)) ^ 6.0)
gain = base - (base - 1.0) * m
```

Packed RGB channels are multiplied by `gain` and rounded.

Vertical cutoff helper:

```text
H(x) = abs(1.0 - (1.0415 - pow(abs(sin(-0.18912*x + 9.764))
                               + abs(sin(-0.037167584307152175*x + 5.896)), 0.1)) ^ 6.0)
```

Callers use roughly `height - 0.05*H(0x4D5238)` as a small nonlinear setup limit.

## 4. Palette file and palette interpolation

### 4.1 Palette object layout

Palette node record: 8 bytes.

| Offset | Meaning |
|---:|---|
| `+0x00` | position in circular `0..1023` domain |
| `+0x04` | red byte |
| `+0x05` | green byte |
| `+0x06` | blue byte |

Palette object:

| Offset | Meaning |
|---:|---|
| `+0x00` | file `field_A`: circular phase/rotation offset |
| `+0x38` | transient palette-editor preview channel/mode |
| `+0x39` | transient preview-valid/dirty flag |
| `+0x3A` | file `field_B`: enabled flag |
| `+0x3B...` | inline palette name string |

### 4.2 `field_A` semantics

Dense palette sampling uses:

```text
sample = round((index / count) * 1024.0 + 1024.0 - field_A) & 0x3ff
```

So `field_A` is a circular palette rotation / phase origin in the same `0..1023` domain as control points.

### 4.3 Dense interpolation (`0x414AD4`)

Input nodes are sorted by circular position and treated on a ring.  A single-node palette fills the whole output with that node's color.

For each segment, and for each channel independently, the function selects four nodes:

- previous
- current `(x0,y0)`
- next `(x1,y1)`
- next-next

Let:

```text
m0 = (y0 - y_prev) / max(1, abs(x0 - x_prev))
m1 = (y1 - y0)   / max(1, abs(x1 - x0))
m2 = (y2 - y1)   / max(1, abs(x2 - x1))
a  = 0.5 * (tanh(4.0 * (abs(m0 - m1) - 0.8)) + 1.0)
b  = 0.5 * (tanh(4.0 * (abs(m1 - m2) - 0.8)) + 1.0)
d0 = (1-a)*0.5*(m0+m1) + a*m1
d1 = (1-b)*0.5*(m1+m2) + b*m1
dx = x1 - x0
```

Hermite-like cubic on local coordinate `s = phase - x0`:

```text
A = (d0 + d1 - 2*(y1-y0)/dx) / (dx*dx)
B = 3*(y1-y0)/(dx*dx) - (2*d0+d1)/dx
P(s) = A*s^3 + B*s^2 + d0*s + y0
```

The temporary curve is then linearly resampled to the requested output count:

```text
src = j * (M / N)
i0  = floor(src)
t   = src - i0
out[j] = round(clamp((1-t)*tmp[i0] + t*tmp[min(i0+1,M-1)], 0, 255))
```

If the caller requests a very large output count, the temporary count is capped at `501`; otherwise it follows the output count below `8016`.

An alternate flag selects geometrically shrinking temporary-sample steps with factor `0.9955000281` instead of uniform stepping.

## 5. Pattern state structure

The renderer state starts with harmonic profile coefficients and continues with randomized scalar-field parameters.

### 5.1 Harmonic/profile header

| Offset | Meaning |
|---:|---|
| `+0x00 + 8*k` | harmonic amplitude coefficient, `k=0..3` |
| `+0x04 + 8*k` | harmonic phase/phase-scale coefficient, `k=0..3` |
| `+0x20` | start of 4000-float profile array after profile generation |

### 5.2 Main fields

| Offset | Meaning / construction |
|---:|---|
| `+0x3EA0` | `2*pi * signed_rand()` |
| `+0x3EA4` | rounded harmonic frequency multiplier |
| `+0x3EA8` | boolean from sign of `signed_rand()` |
| `+0x3EAC` | profile contrast/depth coefficient |
| `+0x3EB0` | harmonic falloff exponent, `{0.4, 4.0}` |
| `+0x3EB4` | caller-supplied final profile amplitude scale |
| `+0x3EB8` | ribbons sine-gate frequency coefficient |
| `+0x3EBC` | `2.0 + 2.0*signed_rand()` |
| `+0x3EC0/+0x3EC1` | random coordinate sign booleans |
| `+0x3EC4` | second raster switch selector, `0..9` family |
| `+0x3EC8` | third raster switch selector, `0..9` family |
| `+0x3ECC` | late blend coefficient, `0.01*(rand%100)` |
| `+0x3ED0/+0x3ED4` | coordinate-warp support values |
| `+0x3ED8..+0x3F3C` | coefficient banks for second/third switches |
| `+0x3F40/+0x3F44` | signed cosine/power coefficients |
| `+0x3F48..+0x3F5C` | first-switch coefficient bank |
| `+0x3F60` | optional angular stripe count/scale |
| `+0x3F64/+0x3F70` | vertical perturbation coefficients |
| `+0x3F68/+0x3F6C/+0x3F74` | coordinate/style support scalars |
| `+0x3F78` | selected symmetry/direction inversion mode |
| `+0x3F79` | phase perturbation gate |
| `+0x3F7C` | coordinate warp selector, `0..3` |
| `+0x3F80` | coordinate blend gate |
| `+0x3F81` | optional stripe/radius blend gate |
| `+0x3F84` | six-preset quadratic warp bank |
| `+0x3FA8..+0x3FAC` | stored spatial/asymmetry double |
| `+0x3FB0` | Ribbons style byte |
| `+0x3FB1` | Mesh style byte |
| `+0x3FB2` | Rhombuses style byte |
| `+0x3FB4` | ribbons threshold/duty control |
| `+0x3FB8` | ribbons stripe frequency control |
| `+0x3FBC` | first raster switch selector, `0..8` |
| `+0x3FC0` | late profile-coordinate combiner selector, `0..5` |

### 5.3 Random helpers

```text
signed_rand() ~= 1.0 - 2.0 * (rand15 / 32768.0)   // range about (-1, 1]
min_c  = min_complexity
span_c = max_complexity - min_complexity
r      = rand % 100
```

### 5.4 Coefficient-bank construction

```text
3F64 = 0.15 + 0.0015*r
3F70 = ((1.0 - 0.02*r) * (pi/4)) + (pi/2)
3F74 = 0.005*r
3F58 = 0.5 + 0.005*r
3F5C in {0.8, 1.0, 1.5, 2.0}
3F48/3F4C = 0.1*min_c + 0.001*span_c*r
3F50 = 0.2 + 0.03*min_c + 0.0003*span_c*r
3F54 = 0.05*min_c + 0.0005*span_c*r
3F40/3F44 = +/- (0.2 + 0.008*r)
3ECC = 0.01*r
3F68 = either 0.1 + 0.0015*r, or forced 0.25
3F6C = -1.0 + 0.02*r
```

Main second/third switch banks:

```text
3ED8 / 3F0C = 1.0 + 0.5*min_c + 0.005*span_c*r
3EDC / 3F10 = min_c + 0.01*span_c*r
3EE0 / 3F14 = 0.1 + 0.39*min_c + 0.0039*span_c*r
3EE4 / 3F18 = either 3.0 + 0.8*min_c + 0.008*span_c*r
              or     20.0 + 2.0*min_c + 0.02*span_c*r
3EE8 / 3F1C = same family, independent branch
3EEC / 3F20 / 3EF0 / 3F24 = 2.0 + 0.4*min_c + 0.004*span_c*r
3EF4 / 3F28 = 1.0 + 2.0*min_c + 0.02*span_c*r
3EF8 / 3F2C = min_c + (rand % round(2.0*span_c))
3EFC / 3F30 = (5 + rand%20) * 32768
3F00 / 3F34 = 1.0 + 0.07*r
3F04 / 3F38 = 0.005*r
3F08 / 3F3C = 0.04*r
```

Profile/ribbons fields:

```text
3EA4 = round(1.0 + min_c + 0.01*span_c*(rand%100))
3EA8 = signed_rand() > 0 ? 1 : 0
3EAC = 0.01 + 0.05 * (min_relief + relief_span*relief_rand)
3EB8 = 0.2 + 2.0*min_c + 0.02*span_c*(rand%100)
3EBC = 2.0 + 2.0*signed_rand()
3FB4 = either 0.2 + 0.005*r, or 1.3 + 0.005*r
3FB8 = 2.0 + 0.4*min_c + 0.004*span_c*r
```

## 6. Quadratic coordinate warp

`0x427D08` chooses one of six quadratic presets at `state + 0x3F84`.

`0x427F38` evaluates:

```text
u' = a0 + 0.5 * (c0*v^2 + 2*c1*u*v + c2*u^2)
v' = a1 + 0.5 * (d0*v^2 + 2*d1*u*v + d2*u^2)
```

`0x427FB0` normalizes each preset by sampling `u,v ∈ {-1,0,1}`, centering each map and scaling it toward a `[-1,1]`-like range.

## 7. Profile generator (`0x40BD74`)

The profile is a 4000-sample float array at `state + 0x20`.

### 7.1 Non-ribbons profile

For sample index `i`:

```text
phi = i * (pi / 2000.0) - pi
sum = 0
for k in 0..3:
    amp   = state[0x00 + 8*k] * pow(k+1, -state->3EB0)
    theta = state->3EA8 * state[0x04 + 8*k] + (state->3EA4 * (k+1)) * phi
    sum  += amp * cos(theta)
raw[i] = 4000.0 * sum
```

If the screen width/global threshold is below `0x1E0`, harmonic pairs `k=2,3` are zeroed by the builder before this stage.

### 7.2 Ribbons profile

For Ribbons (`+0x3FB0 != 0`), the profile is a thresholded sine gate:

```text
g = sin(pi * state->3FB8 * phi) + 1.0
raw[i] = (g < state->3FB4) ? 4000.0 : 0.0
```

### 7.3 Normalization

The generator tracks raw min/max, then conceptually rescales:

```text
norm = (raw[i] - min_raw) / (max_raw - min_raw)
profile[i] = state->3EB4 * (4000.0 + (8000.0 * state->3EAC) * norm)
```

This is the late profile sampled by the rasterizer.

## 8. Raster/scalar-field generation (`0x40A6D8`)

The rasterizer fills a progressive scalar table, not final RGB pixels.  Later stages colorize/remap it.

### 8.1 Per-pixel coordinates

For pixel `(x,y)` in the active work area:

```text
u = (x - W*0.5) / (W*0.5)
v = (y - H*0.5) / (H*0.5)

if state->3EC0: u = -u
if state->3EC1: v = -v

if state->3F79:
    v = cos(pi * v * state->3F64 + state->3F70)

if state->3F80:
    u = 0.5 * (u + v)
    // unblended v remains available to later terms

angle_norm = abs(atan2(u, v)) * (1/pi)
radius     = (1/sqrt(2)) * (0.45 + 0.9999 * hypot(u, v))
```

Coordinate warp selector `+0x3F7C`:

- `0`: keep coordinates.
- `1`: primary coordinate becomes a clamped power-law function of `abs(u)+0.45`.
- `2`: apply the quadratic warp helper.
- `3`: primary coordinate becomes a sine-LUT sample.

Optional angular stripe coordinate:

```text
if state->3F60 != 0 and state->3FBC != 8:
    stripe = 2.0 * abs(frac(state->3F60 * angle_norm) - 0.5)
    if state->3F81 && state->3F60 < 5.0 && state->3EC4 != 9 && state->3EC8 != 9 && !state->3F80:
        radius = 0.5 * (radius + stripe)
```

### 8.2 First switch: `+0x3FBC`

This produces the first local scalar term `S1` (`[ebp-0x10]` in analysis notes).  Cases:

| Case | Family |
|---:|---|
| `0` | no extra modulation except optional inversion |
| `1` | `S1 *= 1/(1 + abs(sin_lut(index)))`, index uses `+0x3F48/+0x3F4C`, `S1`, and angular auxiliary |
| `2` | similar normalized sine-LUT branch with alternate phase construction |
| `3` | product of sine/cosine LUT terms, then reciprocal normalization; uses `+0x3F50/+0x3F54` |
| `4` | cosine-LUT branch driven by product of first-term and angular auxiliary coordinates |
| `5` | `log(1+S1)` feeds sine-LUT/absolute reciprocal normalization |
| `6` | affine blend: `S1 *= (1-s) + abs(sin_lut(index))*s`, `s=+0x3F58` |
| `7` | coordinate-product cosine-LUT reciprocal branch using `65536` scaling |
| `8` | coordinate-product sine-LUT reciprocal branch using `65536` scaling |

If `+0x3F78` is set, the first result is inverted as `S1 = 1 - S1`.

### 8.3 Second switch: `+0x3EC4`

This produces `S2` (`[ebp-0x20]`).  Coefficients mostly come from `+0x3ED8..+0x3F08`.

| Case | Family |
|---:|---|
| `0` | two phase-driven LUT expressions form an exponent; `pow(abs(u)+0.0001, exponent)`, clamped to `<=1` |
| `1` | product of two LUT samples, reciprocal-normalized |
| `2` | `abs(LUT(...)) / (1 + coeff*S1)` style branch using `+0x3ED8/+0x3EDC` |
| `3` | product of two LUT-derived terms with reciprocal normalization |
| `4` | `log(0.02 + u*v + S1)` drives LUT/reciprocal normalization |
| `5` | LUT-driven affine blend |
| `6/7` | coordinate-product branches with `65536` scaling |
| `8` | ratio branch using `1.2` and `0.2` constants |
| `9` | cosine/power branch using signed coefficient `+0x3F40` |

### 8.4 Pre-third blend

Before the third switch, if `+0x3FC0 == 0`, auxiliary terms are prepared:

```text
aux_a = (1 - e) + e*S2
aux_b = e*S2
where e = state->3ECC
```

Otherwise the defaults are `aux_a = 1.0`, `aux_b = 0.0`.

### 8.5 Third switch: `+0x3EC8`

This produces `S3` (`[ebp-0x24]`).  It mirrors the second switch with the `+0x3F0C..+0x3F3C` bank and can use the auxiliary terms above.

| Case | Family |
|---:|---|
| `0` | same `pow(abs(u)+0.0001, exponent)` family as second switch, using `+0x3F30..+0x3F3C` |
| `1` | product of two LUT samples, reciprocal-normalized, with extra auxiliary multiplication |
| `2` | `abs(LUT(...)) / (1 + coeff*aux)` using `+0x3F0C/+0x3F10` |
| `3` | product of LUT terms with extra auxiliary coupling |
| `4` | `log(0.02 + aux*u*v + S1)` branch using `+0x3F14` |
| `5` | LUT-driven affine blend using `+0x3F28/+0x3F2C` |
| `6/7` | ratio branches using `+0x3F20/+0x3F24`, `1.2`, `0.2`, `5.0`, `0.5` |
| `8` | cosine/power branch using signed coefficient `+0x3F44` |
| `9` | falls through to style/late-selector handoff |

### 8.6 Style gating and late selector `+0x3FC0`

Mesh/Rhombuses add gates before the final profile-coordinate combiner.

Rhombuses gate:

```text
a = sin_lut[round(100 * angle_aux * 32768) & 0xffff]
b = sin_lut[round(50 * S1*S1 * 32768) & 0xffff]
late_selector_runs only if b > a
```

Mesh gate:

```text
a = sin_lut[round(100 * angle_aux * 32768) & 0xffff]
b = sin_lut[round(50 * S1 * 32768) & 0xffff]
late_selector_runs only if abs(a - b) >= 0.25
```

If neither Mesh nor Rhombuses is active, the late selector always runs.

Let:

```text
e = state->3ECC
P = 3998.0
```

Then `+0x3FC0` computes the final profile lookup coordinate:

| Case | Formula |
|---:|---|
| `0` | `coord = P * S3` |
| `1` | `coord = P * (e*S2 + (1-e)*S3)` |
| `2` | `coord = P / (1 + 30*(e*S2 + (1-e)*S3))` |
| `3` | `coord = P / (1 + 10*e*S2*S3)` |
| `4` | `coord = (P*S2) / (1 + 10*e*S3)` |
| `5` | `coord = P * pow(S2, (1+e)*S3)` |

If the style gate fails, the rasterizer skips this combiner and retains the earlier coordinate term.

### 8.7 Profile sampling and scalar output

```text
i = round(coord)
frac = coord - i

if !Ribbons:
    profile_value = (1-frac)*profile[i] + frac*profile[i+1]
else:
    profile_value = profile[i]

amplitude = pixel_count * S1
scalar = amplitude * profile_value
```

The scalar is written into the table pointed to by `0x4D1498`; running min/max are tracked in `0x4D1484/0x4D1488`.

## 9. Gamma/pixel/output pipeline

`0x40DBF8` builds a 256-entry gamma table:

```text
gamma_lut[i] = round(255 * pow(i/255.0, gamma))
```

`0x4072D0` processes exactly `16000` pixels into a work structure with parallel buffers:

| Offset | Buffer |
|---:|---|
| `+0x20` | source/intermediate packed 32-bit RGBx |
| `+0xFAA0` | gamma-corrected packed 32-bit RGBx |
| `+0x1F520` | packed 24-bit RGB |
| `+0x2B100` | packed 16-bit output |
| `+0x32E40` | 32-bit copy/output buffer |
| `+0x428C0` | tightly packed RGB triplets |
| `+0x4E4A0` | packed 16-bit words from gamma-remapped channels |

The 16-bit converter recognizes RGB565 masks `0xF800/0x07E0/0x001F` and 5:5:5 masks `0x7C00/0x03E0/0x001F`.

## 10. Implementation guidance for a clone

Minimum credible clone path:

1. Parse palettes as node rings with `field_A` rotation and `field_B` enabled flag.
2. Generate dense RGB palettes with the cubic/tangent-damped interpolation above.
3. Implement the random parameter builder with complexity/relief controls and style selection.
4. Build the 4000-sample profile array.
5. Evaluate the raster scalar field through the three switch families and late profile sampler.
6. Normalize/colorize the scalar table through the palette and gamma pipeline.

For a first visual clone, the case-family formulas can be implemented at family level.  For maximum fidelity, finish exact pseudocode for every `+0x3EC4` and `+0x3EC8` case from the disassembly windows around `0x40AE1A..0x40B398` and `0x40B42E..0x40B9C6`.

## 11. Remaining uncertainty register

The major architecture is resolved.  Remaining uncertainty is localized:

- Some second/third switch cases are represented here by exact families rather than fully expanded algebraic pseudocode.
- UI names for `0x4D5238` and `0x4D5240` are inferred by consumers rather than directly tied to a readable resource caption.
- Palette interpolation is reconstructed strongly as damped cubic/Hermite, but exact edge-case behavior for duplicate-position nodes should be verified before claiming byte-faithful output.
- Profile normalization should handle `max == min` defensively in a clone; the original's exact degenerate behavior is not fully written out.
