#version 300 es
precision highp float;

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec4 a_color;

uniform mat4 u_view_projection;

out vec3 v_position;
out vec3 v_normal;
out vec4 v_color;

void main() {
    v_position = a_position;
    v_normal = a_normal;
    v_color = a_color;
    gl_Position = u_view_projection * vec4(a_position, 1.0);
}
