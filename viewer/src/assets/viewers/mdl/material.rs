//! Getting from the name a mesh gives its material to something the shader can bind.

use std::io::Cursor;

use anyhow::Result;
use half::f16;
use ironworks::file::{File, mtrl};

use super::gpu::TABLE_COLUMNS;

/// What the shader does with a texture. A material names its samplers by hash, and the two shader
/// families the browser meets most name the same three roles differently.
#[derive(Clone, Copy)]
pub enum Role {
    Normal,
    Index,
    Mask,
    Diffuse,
}

/// Which set of meanings a material's textures carry. Every family binds the same four sampler
/// slots, so the slot a texture arrives in does not say what its channels are.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// A mask map: red scales the diffuse color, green the specular color, blue its strength. The
    /// color table is indexed by the index map.
    Character,
    /// The three-texture set the game keeps a compatibility path for. The mask slot holds a
    /// specular map, whose green scales the specular color and blue its strength, and the color
    /// table is indexed by the normal map's alpha.
    Legacy,
    /// A specular map, of which only the red channel means what a mask's does.
    Background,
}

const ROLES: [(u32, Role); 7] = [
    (0x0C5E_C1F1, Role::Normal),
    (0xAAB4_D9E9, Role::Normal),
    (0x565F_8FD8, Role::Index),
    (0x8A4E_82B6, Role::Mask),
    (0x1BBC_2F12, Role::Mask),
    (0x1153_06BE, Role::Diffuse),
    (0x1E6F_EF9C, Role::Diffuse),
];

/// Material constants, by the crc32 of their name, with what a package that declares one leaves it
/// at. A `.shpk` carries the defaults but not the names, so the names come from Meddle's table.
const ALPHA_THRESHOLD: u32 = 0x29AC_0223;
const DIFFUSE_COLOR: u32 = 0x2C2A_34DD;
const EMISSIVE_COLOR: u32 = 0x38A6_4362;
const NORMAL_SCALE: u32 = 0xB554_5FBB;

/// What to clip at when a character material leaves its own threshold at zero. Hair and eyelashes
/// are authored as opaque quads with the cutout in the normal map's blue channel, so without a
/// floor they draw as rectangles.
const CUTOUT: f32 = 0.5;

/// Bit 0 of a material's shader flags.
const HIDE_BACKFACES: u32 = 1;

pub struct Material {
    shader: String,
    family: Family,
    textures: [Option<String>; 4],
    alpha_threshold: f32,
    diffuse: [f32; 3],
    emissive: [f32; 3],
    normal_scale: f32,
    cull: bool,
    /// Taken once, when the color table is handed to the context.
    table: Option<Vec<f32>>,
    rows: usize,
}

impl Material {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let material = mtrl::Material::read(Cursor::new(bytes.to_vec()))?;
        let shader = material.shader().to_owned();

        let mut textures: [Option<String>; 4] = Default::default();
        for sampler in material.samplers() {
            let Some(role) = ROLES
                .iter()
                .find(|(id, _)| *id == sampler.id())
                .map(|(_, role)| *role)
            else {
                continue;
            };
            let Some(texture) = sampler
                .texture_index()
                .and_then(|index| material.textures().get(usize::from(index)))
            else {
                continue;
            };
            textures[role as usize] = Some(texture.path().to_owned());
        }

        let constant = |id: u32| {
            material
                .constants()
                .iter()
                .find(|constant| constant.id() == id)
                .and_then(|constant| material.constant_values(constant))
        };
        let declared = constant(ALPHA_THRESHOLD)
            .and_then(|values| values.first().copied())
            .unwrap_or(0.0);
        let triple = |id, fallback: [f32; 3]| {
            constant(id)
                .and_then(|values| values.first_chunk::<3>().copied())
                .unwrap_or(fallback)
        };
        let diffuse = triple(DIFFUSE_COLOR, [1.0; 3]);
        let emissive = triple(EMISSIVE_COLOR, [0.0; 3]);
        let normal_scale = constant(NORMAL_SCALE)
            .and_then(|values| values.first().copied())
            .unwrap_or(1.0);
        // The compatibility path is the one that still binds a diffuse map, having no color table
        // row to take a diffuse color from.
        let family =
            if shader == "characterlegacy.shpk" && textures[Role::Diffuse as usize].is_some() {
                Family::Legacy
            } else if shader.starts_with("character")
                || matches!(shader.as_str(), "hair.shpk" | "skin.shpk" | "iris.shpk")
            {
                Family::Character
            } else {
                Family::Background
            };
        // Only the character families hide a cutout in the normal map; a bg normal map's third
        // channel is something else, and clipping on it would erase the surface.
        let cutout = family != Family::Background && textures[Role::Normal as usize].is_some();
        let alpha_threshold = match cutout {
            true => declared.max(CUTOUT),
            false => declared,
        };

        let (table, rows) = match material.color_table() {
            Some(table) => (pack(table), table.rows()),
            None => (None, 0),
        };

        Ok(Self {
            shader,
            family,
            textures,
            alpha_threshold,
            diffuse,
            emissive,
            normal_scale,
            cull: material.shader_flags() & HIDE_BACKFACES != 0,
            table,
            rows,
        })
    }

    pub fn texture(&self, role: Role) -> Option<&String> {
        self.textures[role as usize].as_ref()
    }

    pub fn family(&self) -> Family {
        self.family
    }

    pub fn textures(&self) -> impl Iterator<Item = &String> {
        self.textures.iter().flatten()
    }

    pub fn alpha_threshold(&self) -> f32 {
        self.alpha_threshold
    }

    pub fn diffuse(&self) -> [f32; 3] {
        self.diffuse
    }

    pub fn emissive(&self) -> [f32; 3] {
        self.emissive
    }

    pub fn normal_scale(&self) -> f32 {
        self.normal_scale
    }

    pub fn cull(&self) -> bool {
        self.cull
    }

    /// Kept rather than handed over, so a detail level built after this material arrived can be
    /// given it too.
    pub fn table(&self) -> Option<&[f32]> {
        self.table.as_deref()
    }

    pub fn summary(&self) -> String {
        let named = self.textures.iter().flatten().count();
        match self.rows {
            0 => format!("{}, {named} textures", self.shader),
            rows => format!("{}, {named} textures, {rows} color rows", self.shader),
        }
    }
}

/// The color table as the fragment shader reads it: four RGBA texels a row, grouping the fields
/// that are used together. The game's own eight-texel layout carries several more, none of which
/// this shading model has anything to do with.
fn pack(table: &mtrl::ColorTable) -> Option<Vec<f32>> {
    let rows = table.rows();
    if rows == 0 {
        return None;
    }
    // Neither layout stores the specular exponent where the other does, and only the wider one
    // states a roughness at all.
    let exponent = match table.kind() {
        mtrl::ColorTableKind::Extended => 3,
        _ => 7,
    };
    let mut values = Vec::with_capacity(rows * TABLE_COLUMNS as usize * 4);
    for index in 0..rows {
        let row = table.row_values(index)?;
        let shininess = match row.roughness {
            0.0 => f32::from(f16::from_bits(*table.row(index)?.get(exponent)?)),
            roughness => ((1.0 - roughness) * 7.0).exp2(),
        };
        values.extend(row.diffuse);
        values.push(shininess.clamp(1.0, 128.0));
        values.extend(row.specular);
        values.push(row.metalness);
        values.extend(row.emissive);
        values.push(row.sheen_rate);
        values.extend([row.sheen_tint, row.sheen_aperture, 0.0, 0.0]);
    }
    Some(values)
}

/// The file a material name points at. Character models name theirs by filename alone, against a
/// directory the name itself spells out; everything else states a whole path.
pub fn path(name: &str) -> Option<String> {
    let name = name.trim_start_matches('/');
    if name.contains('/') {
        return Some(name.to_owned());
    }
    let stem = name.strip_prefix("mt_")?;
    let kind = stem.as_bytes().first().copied()? as char;
    let set = stem.as_bytes().get(5).copied()? as char;
    let body: u32 = stem.get(1..5)?.parse().ok()?;
    let part: u32 = stem.get(6..10)?.parse().ok()?;
    // The variant is an imc lookup the browser has no way to make from a path alone, and the base
    // one is what every other tool falls back to.
    let directory = match (kind, set) {
        ('c', 'e') => format!("chara/equipment/e{part:04}/material/v0001"),
        ('c', 'a') => format!("chara/accessory/a{part:04}/material/v0001"),
        ('c', 'b') => format!("chara/human/c{body:04}/obj/body/b{part:04}/material/v0001"),
        ('c', 'h') => format!("chara/human/c{body:04}/obj/hair/h{part:04}/material/v0001"),
        ('c', 't') => format!("chara/human/c{body:04}/obj/tail/t{part:04}/material/v0001"),
        ('c', 'f') => format!("chara/human/c{body:04}/obj/face/f{part:04}/material"),
        ('c', 'z') => format!("chara/human/c{body:04}/obj/zear/z{part:04}/material"),
        ('d', 'e') => format!("chara/demihuman/d{body:04}/obj/equipment/e{part:04}/material/v0001"),
        ('m', 'b') => format!("chara/monster/m{body:04}/obj/body/b{part:04}/material/v0001"),
        ('w', 'b') => format!("chara/weapon/w{body:04}/obj/body/b{part:04}/material/v0001"),
        _ => return None,
    };
    Some(format!("{directory}/{name}"))
}
