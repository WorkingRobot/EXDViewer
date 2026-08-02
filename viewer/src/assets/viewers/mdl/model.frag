#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;

in vec3 v_position;
in vec3 v_normal;
in vec4 v_tangent;
in vec2 v_uv;
in vec4 v_color;

uniform vec3 u_eye;
uniform vec3 u_lights[3];
uniform sampler2D u_normal_map;
uniform sampler2D u_index_map;
uniform sampler2D u_mask_map;
uniform sampler2D u_diffuse_map;
uniform sampler2D u_table;
uniform int u_have;
uniform int u_family;
uniform int u_mesh;
uniform float u_alpha_threshold;
uniform vec3 u_diffuse_color;
uniform vec3 u_emissive_color;
uniform float u_normal_scale;
uniform float u_table_rows;
uniform int u_debug;

out vec4 fragColor;

const int HAVE_NORMAL = 1;
const int HAVE_INDEX = 2;
const int HAVE_MASK = 4;
const int HAVE_DIFFUSE = 8;
const int HAVE_TABLE = 16;

const int CHARACTER = 0;
const int LEGACY = 1;
const int BACKGROUND = 2;

const int SHOW_NORMAL = 1;
const int SHOW_UV = 2;
const int SHOW_GEOMETRY = 3;
const int SHOW_TANGENT = 4;
const int SHOW_BITANGENT = 5;
const int SHOW_HANDEDNESS = 6;
const int SHOW_COLOR = 7;
const int SHOW_ALPHA = 8;
const int SHOW_MESH = 9;

/// Drawn where a tangent view has no tangent to draw, since a mesh that declares none arrives with
/// a zero one rather than being dropped.
const vec3 MISSING = vec3(1.0, 0.0, 1.0);

bool has(int flag) {
	return (u_have & flag) != 0;
}

vec3 to_linear(vec3 color) {
	return color * color;
}

vec3 to_display(vec3 color) {
	return sqrt(max(color, vec3(0.0)));
}

/// Narkowicz's fit of the ACES curve. A plain reciprocal also brings the lights back under one,
/// but takes the midtones and the highlight saturation down with it.
vec3 tone(vec3 color) {
	vec3 shouldered = color * (2.51 * color + 0.03);
	return clamp(shouldered / (color * (2.43 * color + 0.59) + 0.14), 0.0, 1.0);
}

vec3 hue(float turns) {
	vec3 wrapped = fract(turns + vec3(0.0, 0.66667, 0.33333)) * 6.0;
	return clamp(abs(wrapped - 3.0) - 1.0, 0.0, 1.0);
}

/// Where in the color table a pixel lands, as a row pair and how far between them it sits. A
/// character reads the pair from the index map's red in seventeenths and blends across it with the
/// green; the compatibility path has no index map and reads a single row from the normal map's
/// alpha, over the sixteen rows a table held before it was widened.
void pick_rows(float alpha, out int lower, out int upper, out float blend) {
	int rows = int(u_table_rows);
	lower = 0;
	upper = 0;
	blend = 0.0;
	if (rows < 2) {
		return;
	}
	if (u_family == LEGACY) {
		float position = clamp(alpha, 0.0, 1.0) * float(min(rows, 16) - 1);
		lower = int(position);
		blend = position - float(lower);
	} else if (has(HAVE_INDEX)) {
		vec2 id = texture(u_index_map, v_uv).rg;
		lower = int((255.0 * id.r + 8.0) / 17.0) * 2;
		blend = clamp(1.0 - id.g, 0.0, 1.0);
	}
	lower = clamp(lower, 0, rows - 1);
	upper = min(lower + 1, rows - 1);
}

vec4 table_texel(int column, int lower, int upper, float blend) {
	vec4 low = texelFetch(u_table, ivec2(column, lower), 0);
	vec4 high = texelFetch(u_table, ivec2(column, upper), 0);
	return mix(low, high, blend);
}

void main() {
	if (u_debug == SHOW_MESH) {
		fragColor = vec4(hue(float(u_mesh) * 0.618034), 1.0);
		return;
	}

	vec3 geometric = normalize(v_normal);
	if (!gl_FrontFacing) {
		geometric = -geometric;
	}
	vec3 view = normalize(u_eye - v_position);

	vec3 across = v_tangent.xyz - geometric * dot(geometric, v_tangent.xyz);
	bool framed = dot(across, across) > 1e-6;
	vec3 tangent = framed ? normalize(across) : vec3(0.0);
	vec3 bitangent = cross(geometric, tangent) * sign(v_tangent.w);

	vec3 normal = geometric;
	vec4 sampled = vec4(0.5, 0.5, 1.0, 0.0);
	if (has(HAVE_NORMAL)) {
		sampled = texture(u_normal_map, v_uv);
		if (framed) {
			vec2 xy = (sampled.xy * 2.0 - 1.0) * u_normal_scale;
			float z = sqrt(max(1.0 - dot(xy, xy), 1e-4));
			normal = normalize(tangent * xy.x + bitangent * xy.y + geometric * z);
		}
	}

	float opacity = v_color.a;
	// Only a character normal map carries opacity here; a background one has a third normal channel
	// in the same place.
	if (has(HAVE_NORMAL) && u_family != BACKGROUND) {
		opacity *= sampled.b;
	}
	if (opacity < u_alpha_threshold) {
		discard;
	}

	if (u_debug != 0) {
		vec3 shown = vec3(0.0);
		if (u_debug == SHOW_NORMAL) {
			shown = normal * 0.5 + 0.5;
		} else if (u_debug == SHOW_UV) {
			shown = vec3(fract(v_uv), 0.0);
		} else if (u_debug == SHOW_GEOMETRY) {
			shown = geometric * 0.5 + 0.5;
		} else if (u_debug == SHOW_TANGENT) {
			shown = framed ? tangent * 0.5 + 0.5 : MISSING;
		} else if (u_debug == SHOW_BITANGENT) {
			shown = framed ? bitangent * 0.5 + 0.5 : MISSING;
		} else if (u_debug == SHOW_HANDEDNESS) {
			shown = v_tangent.w > 0.0 ? vec3(0.1, 0.5, 0.9)
				: (v_tangent.w < 0.0 ? vec3(0.9, 0.6, 0.1) : MISSING);
		} else if (u_debug == SHOW_COLOR) {
			shown = v_color.rgb;
		} else if (u_debug == SHOW_ALPHA) {
			shown = vec3(v_color.a);
		}
		fragColor = vec4(shown, 1.0);
		return;
	}

	vec3 albedo = vec3(0.72);
	vec3 specular = vec3(1.0);
	vec3 emissive = vec3(0.0);
	float strength = 0.3;
	float shininess = 20.0;
	float metalness = 0.0;
	float sheen_rate = 0.0;
	float sheen_tint = 0.0;
	float sheen_aperture = 4.0;

	if (has(HAVE_TABLE)) {
		int lower;
		int upper;
		float blend;
		pick_rows(sampled.a, lower, upper, blend);
		vec4 first = table_texel(0, lower, upper, blend);
		vec4 second = table_texel(1, lower, upper, blend);
		vec4 third = table_texel(2, lower, upper, blend);
		vec4 fourth = table_texel(3, lower, upper, blend);
		albedo = first.rgb;
		shininess = max(first.a, 1.0);
		specular = second.rgb;
		metalness = second.a;
		emissive = third.rgb;
		sheen_rate = third.a;
		sheen_tint = fourth.r;
		sheen_aperture = max(fourth.g, 1.0);
		strength = 1.0;
	}

	if (has(HAVE_DIFFUSE)) {
		vec3 diffuse = to_linear(texture(u_diffuse_map, v_uv).rgb);
		// A compatibility row states a diffuse color of its own beside the map's, and taking both
		// lights the surface at the product of two albedos.
		albedo = u_family == LEGACY ? diffuse : albedo * diffuse;
	}

	if (has(HAVE_MASK)) {
		vec3 mask = texture(u_mask_map, v_uv).rgb;
		if (u_family == BACKGROUND) {
			strength *= mask.r;
		} else {
			vec3 squared = mask * mask;
			if (u_family == CHARACTER) {
				albedo *= squared.r;
			}
			specular *= squared.g;
			strength *= squared.b;
		}
	}

	vec3 mirror = reflect(-view, normal);
	vec3 diffuse_light = vec3(0.0);
	vec3 specular_light = vec3(0.0);
	// A key that outweighs everything else by about five to one, which is most of what reads as
	// lighting rather than as a flat wash.
	const vec3 KEY_COLOR = vec3(1.35, 1.28, 1.16);
	const vec3 FILL_COLOR = vec3(0.30, 0.36, 0.46);
	const vec3 RIM_COLOR = vec3(0.45, 0.48, 0.58);
	vec3 colors[3];
	colors[0] = KEY_COLOR;
	colors[1] = FILL_COLOR;
	colors[2] = RIM_COLOR;
	for (int i = 0; i < 3; ++i) {
		diffuse_light += colors[i] * max(dot(normal, u_lights[i]), 0.0);
		specular_light += colors[i] * pow(max(dot(mirror, u_lights[i]), 0.0), shininess);
	}

	// A hemisphere fill, so a surface facing away from every light still reads as a surface.
	const vec3 SKY = vec3(0.20, 0.23, 0.29);
	const vec3 GROUND = vec3(0.09, 0.085, 0.08);
	vec3 ambient = mix(GROUND, SKY, normal.y * 0.5 + 0.5);

	albedo *= u_diffuse_color;
	emissive += u_emissive_color;
	vec3 tint = mix(vec3(1.0), albedo, metalness);
	vec3 lit = albedo * (diffuse_light * (1.0 - metalness * 0.85) + ambient);
	lit += specular_light * specular * tint * strength;

	// An edge the material has no say in, so a silhouette separates from the background whether or
	// not its color table asked for sheen. Gated on the rim light, since a halo that owes nothing
	// to where the lights are reads as a coat of paint.
	float edge = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 3.0);
	lit += RIM_COLOR * edge * max(dot(normal, u_lights[2]), 0.0) * 0.25;

	float fresnel = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), sheen_aperture);
	lit += sheen_rate * fresnel * mix(vec3(1.0), specular, sheen_tint);
	lit += emissive;

	fragColor = vec4(to_display(tone(lit)), 1.0);
}
